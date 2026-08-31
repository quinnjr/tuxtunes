//! The outbound sync pipeline.
//!
//! Phases run in a fixed order, each reporting progress:
//! enumerate, plan, upload, write playlists, prune, finalize.
//! (Transcoding and stats pull-back are declared in
//! [`DevicePhase`](super::events::DevicePhase) but not yet emitted.)
//!
//! Two invariants hold throughout:
//!
//! 1. A manifest row is written only after the object is fully on the
//!    device, so an interrupted run under-claims rather than lies.
//! 2. Only manifest rows are ever deleted, so a file the user put on
//!    the device by hand survives a mirroring sync.

use super::events::{
    DeviceComplete, DevicePhase, DeviceProgress, DeviceWarning, DeviceWarningKind,
};
use super::layout;
use super::manifest::{self, Desired, SyncPlan, KIND_PLAYLIST, KIND_TRACK};
use super::observer::DeviceObserver;
use super::playlists::{self as m3u, PlaylistEntry};
use super::transport::{DevicePath, DeviceTransport, TransportError};
use crate::db::device_objects::{self, NewDeviceObject};
use crate::db::devices::{DeviceRow, SelectionEntry};
use crate::db::tracks::TrackRow;
use crate::db::{albums, playlists as db_playlists, smart, tracks};
use crate::fs::path::TrackFields;
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Chunk size for streaming a track onto the device. Also the interval
/// at which cancellation is observed, so a cancel during a large FLAC
/// is felt promptly rather than at the end of the file.
const COPY_CHUNK: usize = 1024 * 1024;

/// Directory, relative to the device root, holding the `.m3u8` files.
const PLAYLIST_DIR: &str = "Playlists";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("device query failed: {0}")]
    Db(#[source] anyhow::Error),
    #[error("transport failed: {0}")]
    Transport(#[from] TransportError),
    #[error("device is out of space: need {needed} bytes, {free} free")]
    NoSpace { needed: u64, free: u64 },
    #[error("no transport for device kind '{0}' yet")]
    TransportUnavailable(String),
}

fn db_err(e: impl Into<anyhow::Error>) -> EngineError {
    EngineError::Db(e.into())
}

/// Why a track in the selection did not reach the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub track_id: i64,
    pub reason: SkipReason,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    MissingSourceFile,
    UnsupportedCodec,
}

impl SkipReason {
    fn warning_kind(self) -> DeviceWarningKind {
        match self {
            SkipReason::MissingSourceFile => DeviceWarningKind::MissingSourceFile,
            SkipReason::UnsupportedCodec => DeviceWarningKind::UnsupportedCodec,
        }
    }
}

/// Resolve a device's selection into the tracks it names.
///
/// De-duplicated by track id, preserving first-seen order, so a track
/// in three selected playlists is pushed once.
pub async fn resolve_selection(
    engine: &SqliteRawEngine,
    selection: &[SelectionEntry],
) -> Result<Vec<TrackRow>, EngineError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in selection {
        let rows = match entry {
            SelectionEntry::Playlist { id } => db_playlists::tracks_for_regular(engine, *id)
                .await
                .map_err(db_err)?,
            SelectionEntry::Smart { id } => {
                let Some(raw) = db_playlists::get_smart_rule(engine, *id)
                    .await
                    .map_err(db_err)?
                else {
                    continue;
                };
                let rule: smart::SmartRule = serde_json::from_str(&raw).map_err(db_err)?;
                smart::evaluate(engine, &rule).await.map_err(db_err)?
            }
            SelectionEntry::Album {
                album_artist,
                album,
            } => albums::tracks_for_album(engine, album_artist, album)
                .await
                .map_err(db_err)?,
            SelectionEntry::All => {
                tracks::list(engine, i64::MAX, 0, &Default::default(), None)
                    .await
                    .map_err(db_err)?
            }
        };
        for row in rows {
            if seen.insert(row.id) {
                out.push(row);
            }
        }
    }
    Ok(out)
}

/// Render every selected track to its device path and diff against the
/// manifest. Tracks whose file is gone become [`Skip`]s rather than
/// failing the run.
pub async fn build_plan(
    engine: &SqliteRawEngine,
    device: &DeviceRow,
    transport: &dyn DeviceTransport,
) -> Result<(SyncPlan, Vec<Skip>), EngineError> {
    let caps = transport.capabilities();
    let root = DevicePath::new(&device.root_path);
    let selected = resolve_selection(engine, &device.selection).await?;
    let existing = device_objects::list_for_device(engine, device.id)
        .await
        .map_err(db_err)?;

    let mut desired = Vec::with_capacity(selected.len());
    let mut skips = Vec::new();
    // Two tracks can legitimately render to the same path (same album,
    // same title). Suffix the later ones so neither is silently lost.
    let mut taken: HashSet<String> = HashSet::new();

    for row in &selected {
        let source = Path::new(&row.file_path);
        let size = match std::fs::metadata(source) {
            Ok(md) => md.len() as i64,
            Err(_) => {
                skips.push(Skip {
                    track_id: row.id,
                    reason: SkipReason::MissingSourceFile,
                    detail: row.file_path.clone(),
                });
                continue;
            }
        };

        let fields = TrackFields::from_track_row(row, source);
        let rendered = match layout::render(&device.layout_template, &root, &fields, &caps) {
            Ok(p) => p,
            Err(e) => {
                skips.push(Skip {
                    track_id: row.id,
                    reason: SkipReason::UnsupportedCodec,
                    detail: format!("path template failed: {e}"),
                });
                continue;
            }
        };
        let path = dedupe_path(rendered, &mut taken);

        desired.push(Desired {
            track_id: row.id,
            persistent_id: None,
            device_path: path.as_str().to_string(),
            source_hash: row.file_hash.clone(),
            // Phase 1 pushes every track bit-exact. The Phase 3 format
            // policy replaces this with a real decision, and the codec
            // recorded here is what makes that change show up as a
            // replace in the diff.
            encoded_codec: format!("copy:{}", row.kind.as_deref().unwrap_or("unknown")),
            size_bytes: size,
            source_path: row.file_path.clone(),
        });
    }

    Ok((manifest::diff(&desired, &existing), skips))
}

/// Suffix ` (2)`, ` (3)` … until the path is unclaimed this run.
fn dedupe_path(path: DevicePath, taken: &mut HashSet<String>) -> DevicePath {
    if taken.insert(path.as_str().to_string()) {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| DevicePath::new("/"));
    let name = path.file_name().unwrap_or("file");
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    for n in 2..1000 {
        let candidate = parent.join(&format!("{stem} ({n}){ext}"));
        if taken.insert(candidate.as_str().to_string()) {
            return candidate;
        }
    }
    path
}

/// Run one full sync.
pub async fn run(
    engine: &SqliteRawEngine,
    transport: &dyn DeviceTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    cancel: &AtomicBool,
) -> Result<DeviceComplete, EngineError> {
    let mut done = DeviceComplete {
        device_id: device.id,
        ..DeviceComplete::default()
    };

    report(obs, device.id, DevicePhase::Enumerating, 0, 1, "reading device");
    let storage = if transport.capabilities().free_space {
        transport.free_space().ok()
    } else {
        None
    };
    clean_partials(transport, &DevicePath::new(&device.root_path));

    report(obs, device.id, DevicePhase::Planning, 0, 1, "planning");
    let (plan, skips) = build_plan(engine, device, transport).await?;
    done.skipped = skips.len() as u64;
    for skip in &skips {
        warn(
            obs,
            device.id,
            skip.reason.warning_kind(),
            format!("track {}: {}", skip.track_id, skip.detail),
        );
    }

    if let Some(info) = storage {
        if plan.bytes_out > info.free_bytes {
            return Err(EngineError::NoSpace {
                needed: plan.bytes_out,
                free: info.free_bytes,
            });
        }
    }

    let uploaded = upload_phase(engine, transport, obs, device, &plan, cancel, &mut done).await?;

    done.playlists_written =
        playlist_phase(engine, transport, obs, device, &uploaded, cancel).await?;

    done.deleted = prune_phase(engine, transport, obs, device, &plan, cancel).await?;

    report(obs, device.id, DevicePhase::Finalizing, 1, 1, "done");
    crate::db::devices::mark_synced(engine, device.id)
        .await
        .map_err(db_err)?;
    Ok(done)
}

/// Upload every add and replace, recording each in the manifest only
/// after it lands. Returns `track_id -> device_path` for everything
/// now on the device, which the playlist phase needs.
async fn upload_phase(
    engine: &SqliteRawEngine,
    transport: &dyn DeviceTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    plan: &SyncPlan,
    cancel: &AtomicBool,
    done: &mut DeviceComplete,
) -> Result<HashMap<i64, String>, EngineError> {
    done.unchanged = plan.unchanged as u64;

    // Everything already on the device counts as present for playlists.
    let mut present: HashMap<i64, String> = HashMap::new();
    for row in device_objects::list_for_device(engine, device.id)
        .await
        .map_err(db_err)?
    {
        if row.kind == KIND_TRACK {
            if let Some(track_id) = row.track_id {
                present.insert(track_id, row.device_path);
            }
        }
    }

    let work: Vec<&Desired> = plan
        .adds
        .iter()
        .chain(plan.replaces.iter().map(|(_, d)| d))
        .collect();
    let total = work.len() as u64;

    for (i, want) in work.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        report(
            obs,
            device.id,
            DevicePhase::Uploading,
            i as u64,
            total,
            want.device_path.clone(),
        );

        match upload_one(transport, want) {
            Ok(bytes) => {
                device_objects::upsert(
                    engine,
                    &NewDeviceObject {
                        device_id: device.id,
                        kind: KIND_TRACK.into(),
                        track_id: Some(want.track_id),
                        persistent_id: want.persistent_id.clone(),
                        device_path: want.device_path.clone(),
                        object_id: None,
                        source_hash: want.source_hash.clone(),
                        encoded_codec: want.encoded_codec.clone(),
                        size_bytes: want.size_bytes,
                    },
                )
                .await
                .map_err(db_err)?;
                present.insert(want.track_id, want.device_path.clone());
                done.bytes_written += bytes;
                done.added += 1;
            }
            Err(TransportError::NoSpace) => {
                // Everything written so far is recorded and intact;
                // stopping here is the honest outcome.
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::OutOfSpace,
                    format!("ran out of space at {}", want.device_path),
                );
                return Err(EngineError::NoSpace {
                    needed: want.size_bytes.max(0) as u64,
                    free: 0,
                });
            }
            Err(e) => {
                done.skipped += 1;
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::UploadFailed,
                    format!("{}: {e}", want.device_path),
                );
            }
        }
    }

    // `added` counted both adds and replaces; split them for the UI.
    let replaced = plan.replaces.len() as u64;
    done.replaced = replaced.min(done.added);
    done.added -= done.replaced;

    Ok(present)
}

/// Stream one track onto the device, writing to a `.tuxpart` name and
/// renaming into place where the transport supports it, so a partial
/// transfer never occupies the real path.
fn upload_one(transport: &dyn DeviceTransport, want: &Desired) -> Result<u64, TransportError> {
    let final_path = DevicePath::new(&want.device_path);
    let parent = final_path
        .parent()
        .ok_or_else(|| TransportError::NotFound("track has no parent directory".into()))?;
    transport.mkdir_all(&parent)?;

    let use_temp = transport.capabilities().rename;
    let write_path = if use_temp {
        DevicePath::new(&format!("{}.tuxpart", want.device_path))
    } else {
        final_path.clone()
    };

    let mut source = std::fs::File::open(&want.source_path)
        .map_err(|e| TransportError::NotFound(format!("{}: {e}", want.source_path)))?;
    let mut written = 0u64;
    {
        let mut sink = transport.open_write(&write_path, want.size_bytes.max(0) as u64)?;
        let mut buf = vec![0u8; COPY_CHUNK];
        loop {
            let n = std::io::Read::read(&mut source, &mut buf)
                .map_err(|e| TransportError::Other(anyhow::Error::from(e)))?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n])
                .map_err(|e| TransportError::Other(anyhow::Error::from(e)))?;
            written += n as u64;
        }
        sink.flush()
            .map_err(|e| TransportError::Other(anyhow::Error::from(e)))?;
    }

    if use_temp {
        // An overwrite must clear the old object first: not every
        // device's rename replaces an existing name.
        let _ = transport.delete(&final_path);
        transport.rename(&write_path, &final_path)?;
    }
    Ok(written)
}

/// Write one `.m3u8` per selected playlist, listing only tracks that
/// actually reached the device, and register a native playlist object
/// where the transport offers one.
async fn playlist_phase(
    engine: &SqliteRawEngine,
    transport: &dyn DeviceTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    present: &HashMap<i64, String>,
    cancel: &AtomicBool,
) -> Result<u64, EngineError> {
    let caps = transport.capabilities();
    let root = DevicePath::new(&device.root_path);
    let dir = root.join(PLAYLIST_DIR);

    let wanted = selected_playlists(engine, device).await?;
    let total = wanted.len() as u64;
    if total > 0 {
        transport.mkdir_all(&dir)?;
    }

    let mut written_paths = BTreeSet::new();
    let mut count = 0u64;

    for (i, (playlist_id, name, ancestors)) in wanted.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        report(
            obs,
            device.id,
            DevicePhase::Playlists,
            i as u64,
            total,
            name.clone(),
        );

        let rows = playlist_tracks(engine, *playlist_id).await?;
        let entries: Vec<PlaylistEntry> = rows
            .iter()
            .filter_map(|r| {
                present.get(&r.id).map(|p| PlaylistEntry {
                    device_path: DevicePath::new(p),
                    duration_secs: (r.duration_ms.max(0) / 1000) as u64,
                    artist: r.artist.clone().unwrap_or_default(),
                    title: r.title.clone(),
                })
            })
            .collect();

        let file = dir.join(&m3u::playlist_file_name(name, ancestors, &caps));
        let body = m3u::render_m3u8(&entries, &dir);

        match write_text(transport, &file, &body) {
            Ok(()) => {}
            Err(e) => {
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::UploadFailed,
                    format!("{}: {e}", file.as_str()),
                );
                continue;
            }
        }

        if device.write_playlist_objects && caps.playlist_objects {
            let ids: Vec<_> = entries
                .iter()
                .map(|e| super::transport::ObjectId(e.device_path.as_str().to_string()))
                .collect();
            if let Err(e) = transport.create_playlist_object(&file, &ids) {
                // The `.m3u8` is already written; the native object is
                // a bonus for the device's own scanner.
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::PlaylistObjectFailed,
                    format!("{}: {e}", file.as_str()),
                );
            }
        }

        device_objects::upsert(
            engine,
            &NewDeviceObject {
                device_id: device.id,
                kind: KIND_PLAYLIST.into(),
                track_id: None,
                persistent_id: None,
                device_path: file.as_str().to_string(),
                object_id: None,
                source_hash: None,
                encoded_codec: "m3u8".into(),
                size_bytes: body.len() as i64,
            },
        )
        .await
        .map_err(db_err)?;

        written_paths.insert(file.as_str().to_string());
        count += 1;
    }

    // Playlists we wrote on an earlier run but no longer want.
    if device.mirror_deletes && !device.key_is_weak && !cancel.load(Ordering::Relaxed) {
        for row in device_objects::list_for_device(engine, device.id)
            .await
            .map_err(db_err)?
        {
            if row.kind != KIND_PLAYLIST || written_paths.contains(&row.device_path) {
                continue;
            }
            let path = DevicePath::new(&row.device_path);
            if let Err(e) = transport.delete(&path) {
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::DeleteFailed,
                    format!("{}: {e}", row.device_path),
                );
            }
            device_objects::remove_by_id(engine, row.id)
                .await
                .map_err(db_err)?;
        }
    }

    Ok(count)
}

/// Delete orphaned track objects, then any directories we emptied.
async fn prune_phase(
    engine: &SqliteRawEngine,
    transport: &dyn DeviceTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    plan: &SyncPlan,
    cancel: &AtomicBool,
) -> Result<u64, EngineError> {
    // A weak device key could match the wrong hardware; never delete
    // on a guess.
    if !device.mirror_deletes || device.key_is_weak || cancel.load(Ordering::Relaxed) {
        return Ok(0);
    }

    let total = plan.orphans.len() as u64;
    let mut deleted = 0u64;
    let mut parents = BTreeSet::new();

    for (i, row) in plan.orphans.iter().enumerate() {
        report(
            obs,
            device.id,
            DevicePhase::Pruning,
            i as u64,
            total,
            row.device_path.clone(),
        );
        let path = DevicePath::new(&row.device_path);
        match transport.delete(&path) {
            Ok(()) | Err(TransportError::NotFound(_)) => {}
            Err(e) => {
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::DeleteFailed,
                    format!("{}: {e}", row.device_path),
                );
                continue;
            }
        }
        if let Some(p) = path.parent() {
            parents.insert(p.as_str().to_string());
        }
        device_objects::remove_by_id(engine, row.id)
            .await
            .map_err(db_err)?;
        deleted += 1;
    }

    remove_empty_dirs(transport, &parents, &DevicePath::new(&device.root_path));
    Ok(deleted)
}

/// Walk each touched directory upwards, removing those we emptied,
/// stopping at the device root so we never delete it.
fn remove_empty_dirs(
    transport: &dyn DeviceTransport,
    parents: &BTreeSet<String>,
    root: &DevicePath,
) {
    for start in parents {
        let mut cursor = DevicePath::new(start);
        while cursor != *root && cursor.as_str() != "/" {
            match transport.list(&cursor) {
                Ok(children) if children.is_empty() => {
                    if transport.delete(&cursor).is_err() {
                        break;
                    }
                }
                _ => break,
            }
            match cursor.parent() {
                Some(p) => cursor = p,
                None => break,
            }
        }
    }
}

/// Remove `.tuxpart` leftovers from an interrupted earlier run. They
/// hold no manifest row, so nothing else would ever clean them up.
fn clean_partials(transport: &dyn DeviceTransport, root: &DevicePath) {
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(children) = transport.list(&dir) else {
            continue;
        };
        for child in children {
            if child.is_dir {
                stack.push(child.path);
            } else if child.path.as_str().ends_with(".tuxpart") {
                let _ = transport.delete(&child.path);
            }
        }
    }
}

/// The playlists this device's selection names, as
/// `(id, name, ancestor names outermost-first)`.
async fn selected_playlists(
    engine: &SqliteRawEngine,
    device: &DeviceRow,
) -> Result<Vec<(i64, String, Vec<String>)>, EngineError> {
    let wanted: HashSet<i64> = device
        .selection
        .iter()
        .filter_map(|e| match e {
            SelectionEntry::Playlist { id } | SelectionEntry::Smart { id } => Some(*id),
            _ => None,
        })
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let all = db_playlists::list_all(engine).await.map_err(db_err)?;
    let by_id: HashMap<i64, &db_playlists::PlaylistRow> =
        all.iter().map(|p| (p.id, p)).collect();

    Ok(all
        .iter()
        .filter(|p| wanted.contains(&p.id) && p.kind != "folder")
        .map(|p| {
            let mut ancestors = Vec::new();
            let mut cursor = p.parent_id;
            // Bounded by the playlist count, so a parent_id cycle
            // cannot spin forever.
            for _ in 0..all.len() {
                let Some(id) = cursor else { break };
                let Some(parent) = by_id.get(&id) else { break };
                ancestors.push(parent.name.clone());
                cursor = parent.parent_id;
            }
            ancestors.reverse();
            (p.id, p.name.clone(), ancestors)
        })
        .collect())
}

async fn playlist_tracks(
    engine: &SqliteRawEngine,
    playlist_id: i64,
) -> Result<Vec<TrackRow>, EngineError> {
    if let Some(raw) = db_playlists::get_smart_rule(engine, playlist_id)
        .await
        .map_err(db_err)?
    {
        let rule: smart::SmartRule = serde_json::from_str(&raw).map_err(db_err)?;
        return smart::evaluate(engine, &rule).await.map_err(db_err);
    }
    db_playlists::tracks_for_regular(engine, playlist_id)
        .await
        .map_err(db_err)
}

fn write_text(
    transport: &dyn DeviceTransport,
    path: &DevicePath,
    body: &str,
) -> Result<(), TransportError> {
    let mut sink = transport.open_write(path, body.len() as u64)?;
    sink.write_all(body.as_bytes())
        .map_err(|e| TransportError::Other(anyhow::Error::from(e)))?;
    sink.flush()
        .map_err(|e| TransportError::Other(anyhow::Error::from(e)))
}

fn report(
    obs: &dyn DeviceObserver,
    device_id: i64,
    phase: DevicePhase,
    current: u64,
    total: u64,
    message: impl Into<String>,
) {
    obs.progress(&DeviceProgress {
        device_id,
        phase,
        current,
        total,
        message: message.into(),
    });
}

fn warn(
    obs: &dyn DeviceObserver,
    device_id: i64,
    kind: DeviceWarningKind,
    detail: impl Into<String>,
) {
    obs.warning(&DeviceWarning {
        device_id,
        kind,
        detail: detail.into(),
    });
}
