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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Chunk size for streaming a track onto the device. Also the interval
/// at which cancellation is observed, so a cancel during a large FLAC
/// is felt promptly rather than at the end of the file.
const COPY_CHUNK: usize = 1024 * 1024;

/// Deadline for a single metadata operation (list, stat, mkdir, delete,
/// rename, free space). These are fast on a healthy device and hang
/// forever on a wedged mount, so the gap between the two is wide.
///
/// Shortened under `cfg(test)`: the timeout tests drive the same code
/// path, and a paused clock cannot help because tokio will not
/// auto-advance while a `spawn_blocking` thread is still pending. The
/// test value stays far above any in-memory transport operation.
#[cfg(not(test))]
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const METADATA_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a transfer may make *no* progress before the device is
/// declared unresponsive. Deliberately not a total deadline: a large
/// FLAC over MTP legitimately takes minutes, and only a stall is a
/// fault.
#[cfg(not(test))]
const COPY_STALL_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const COPY_STALL_TIMEOUT: Duration = Duration::from_secs(3);

/// Directory, relative to the device root, holding the `.m3u8` files.
const PLAYLIST_DIR: &str = "Playlists";

/// Prefix for the staging name an upload writes to before renaming
/// into place.
///
/// A short fixed name rather than `<final name>.tuxpart`: the final
/// name may already sit exactly at the device's `max_path_bytes`, and
/// appending to it produced an unfixable `ENAMETOOLONG` for every such
/// track.
const PARTIAL_PREFIX: &str = ".tuxpart-";

/// The staging name for this run.
///
/// Unique per run, so the cleanup sweep can tell a leftover from an
/// earlier run apart from the file this run is writing right now — an
/// orphaned sweep (its deadline elapsed, but a blocking task cannot be
/// cancelled) would otherwise delete a transfer in flight.
fn run_partial_name() -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{PARTIAL_PREFIX}{nonce:x}")
}

/// A shared handle to the device, so blocking work can be moved onto
/// the blocking pool without borrowing.
pub type SharedTransport = Arc<dyn DeviceTransport>;

/// Run one blocking transport operation on the blocking pool, with a
/// deadline.
///
/// Transport calls are synchronous by nature — `libmtp` and the
/// filesystem both block — so running them inline would stall the async
/// worker and, on a wedged mount, hang it indefinitely.
///
/// On timeout the blocking task is *orphaned*: a blocked syscall cannot
/// be cancelled from outside. It finishes or dies with the process. We
/// stop waiting, which is what keeps the worker responsive.
async fn op<T, F>(transport: &SharedTransport, what: &str, f: F) -> Result<T, TransportError>
where
    F: FnOnce(&dyn DeviceTransport) -> Result<T, TransportError> + Send + 'static,
    T: Send + 'static,
{
    let transport = Arc::clone(transport);
    let task = tokio::task::spawn_blocking(move || f(transport.as_ref()));
    match tokio::time::timeout(METADATA_TIMEOUT, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(TransportError::Other(anyhow::Error::from(join))),
        Err(_) => Err(TransportError::Timeout(what.to_string())),
    }
}

/// As [`op`], but for work whose failure is not worth propagating.
async fn op_best_effort<F>(transport: &SharedTransport, what: &str, f: F)
where
    F: FnOnce(&dyn DeviceTransport) + Send + 'static,
{
    let transport = Arc::clone(transport);
    let task = tokio::task::spawn_blocking(move || f(transport.as_ref()));
    if tokio::time::timeout(METADATA_TIMEOUT, task).await.is_err() {
        log::warn!("device: {what} timed out");
    }
}

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
    /// The user stopped the run. Everything already transferred is
    /// intact and recorded; `last_sync_at` is deliberately not stamped.
    #[error("sync cancelled")]
    Cancelled,
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
            SelectionEntry::Playlist { id } => {
                match db_playlists::tracks_for_regular(engine, *id).await {
                    Ok(rows) => rows,
                    // The playlist was deleted while still selected.
                    // Nothing prunes `devices.selection`, and failing
                    // hard here would lock the device out of both sync
                    // and preview with no way to clear the entry — the
                    // Smart arm below already skips for the same reason.
                    Err(db_playlists::PlaylistsError::NotFound(_)) => continue,
                    Err(e) => return Err(db_err(e)),
                }
            }
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
            SelectionEntry::All => tracks::list(engine, i64::MAX, 0, &Default::default(), None)
                .await
                .map_err(db_err)?,
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
    transport: &SharedTransport,
) -> Result<(SyncPlan, Vec<Skip>), EngineError> {
    let caps = transport.capabilities();
    let root = DevicePath::new(&device.root_path);
    let selected = resolve_selection(engine, &device.selection).await?;
    let existing = device_objects::list_for_device(engine, device.id)
        .await
        .map_err(db_err)?;

    // One `metadata` call per track adds up on a large selection, so
    // the whole loop goes to the blocking pool rather than stalling the
    // async worker.
    let template = device.layout_template.clone();
    let (desired, skips) =
        tokio::task::spawn_blocking(move || render_desired(&selected, &template, &root, &caps))
            .await
            .map_err(db_err)?;

    Ok((manifest::diff(&desired, &existing), skips))
}

/// Pure, blocking half of [`build_plan`]: stat each source file and
/// render its device path.
fn render_desired(
    selected: &[TrackRow],
    template: &str,
    root: &DevicePath,
    caps: &super::transport::Capabilities,
) -> (Vec<Desired>, Vec<Skip>) {
    let mut desired = Vec::with_capacity(selected.len());
    let mut skips = Vec::new();
    // Two tracks can legitimately render to the same path (same album,
    // same title). Suffix the later ones so neither is silently lost.
    let mut taken: HashSet<String> = HashSet::new();

    for row in selected {
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
        let rendered = match layout::render(template, root, &fields, caps) {
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
            // `diff` sets this on the paths a manifest row claims.
            replaces_existing: false,
        });
    }

    (desired, skips)
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
    // Exhausted the suffixes. Returning `path` would hand back a name
    // already claimed this run, so two tracks would race for it and one
    // would silently overwrite the other; fall back to a timestamp, as
    // `fs::path::resolve_collision` does.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let candidate = parent.join(&format!("{stem} ({ts:x}){ext}"));
    taken.insert(candidate.as_str().to_string());
    candidate
}

/// Run one full sync.
pub async fn run(
    engine: &SqliteRawEngine,
    transport: &SharedTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    cancel: &Arc<AtomicBool>,
) -> Result<DeviceComplete, EngineError> {
    let mut done = DeviceComplete {
        device_id: device.id,
        ..DeviceComplete::default()
    };

    report(
        obs,
        device.id,
        DevicePhase::Enumerating,
        0,
        1,
        "reading device",
    );
    let storage = if transport.capabilities().free_space {
        // A device that will not answer here is one we should not start
        // writing to, so this deadline is load-bearing.
        op(transport, "reading free space", |t| t.free_space())
            .await
            .ok()
    } else {
        None
    };

    // One staging name for the whole run: the cleanup sweep below
    // uses it to leave this run's in-flight transfer alone.
    let partial_name = run_partial_name();

    report(obs, device.id, DevicePhase::Planning, 0, 1, "planning");
    let (plan, skips) = build_plan(engine, device, transport).await?;
    clean_partials(transport, &plan, &partial_name).await;
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

    let uploaded = upload_phase(
        engine,
        transport,
        obs,
        device,
        &plan,
        cancel,
        &mut done,
        &partial_name,
    )
    .await?;

    done.playlists_written =
        playlist_phase(engine, transport, obs, device, &uploaded, cancel).await?;

    done.deleted = prune_phase(engine, transport, obs, device, &plan, cancel).await?;

    // A cancelled run reached only part of the selection. Stamping
    // last_sync_at would make it indistinguishable from a finished one
    // and let the next run treat the remainder as already handled.
    //
    // It is still a success, not a failure: everything transferred is
    // intact and recorded, and the counts describe what really landed.
    if cancel.load(Ordering::Relaxed) {
        done.cancelled = true;
        report(
            obs,
            device.id,
            DevicePhase::Finalizing,
            1,
            1,
            "cancelled".to_string(),
        );
        return Ok(done);
    }

    report(obs, device.id, DevicePhase::Finalizing, 1, 1, "done");
    crate::db::devices::mark_synced(engine, device.id)
        .await
        .map_err(db_err)?;
    Ok(done)
}

/// Upload every add and replace, recording each in the manifest only
/// after it lands. Returns `track_id -> device_path` for everything
/// now on the device, which the playlist phase needs.
#[allow(clippy::too_many_arguments)]
async fn upload_phase(
    engine: &SqliteRawEngine,
    transport: &SharedTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    plan: &SyncPlan,
    cancel: &Arc<AtomicBool>,
    done: &mut DeviceComplete,
    partial_name: &str,
) -> Result<HashMap<i64, String>, EngineError> {
    done.unchanged = plan.unchanged as u64;

    // Everything already on the device counts as present for
    // playlists — except the orphans this run is about to delete, or a
    // playlist would list paths that vanish minutes later.
    let doomed: HashSet<i64> = plan.orphans.iter().filter_map(|r| r.track_id).collect();
    let mut present: HashMap<i64, String> = HashMap::new();
    for row in device_objects::list_for_device(engine, device.id)
        .await
        .map_err(db_err)?
    {
        if row.kind == KIND_TRACK {
            if let Some(track_id) = row.track_id {
                if !doomed.contains(&track_id) {
                    present.insert(track_id, row.device_path);
                }
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

        match upload_one_watched(transport, want, cancel, partial_name).await {
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
            // The user asked to stop; not a failure, and not a skip.
            Err(TransportError::Cancelled) => break,
            // The old copy is gone and the new one did not land. Drop
            // the row so the manifest stops claiming a file that is no
            // longer there, and so the next run re-adds it.
            Err(e @ TransportError::Replaced(..)) => {
                done.skipped += 1;
                if let Some((row_id, _)) = plan
                    .replaces
                    .iter()
                    .find(|(_, d)| d.device_path == want.device_path)
                {
                    device_objects::remove_by_id(engine, *row_id)
                        .await
                        .map_err(db_err)?;
                }
                present.remove(&want.track_id);
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::UploadFailed,
                    format!("{}: {e}", want.device_path),
                );
            }
            // The device is gone. Continuing would burn a full stall
            // window per remaining track — hours on a large selection —
            // and every one of them would fail identically.
            Err(e @ (TransportError::Disconnected | TransportError::Timeout(_))) => {
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::UploadFailed,
                    format!("{}: {e}", want.device_path),
                );
                return Err(EngineError::Transport(e));
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

/// Run one upload on the blocking pool, watched for stalls.
///
/// The copy is inherently blocking, and a wedged mount can park it in a
/// syscall forever. Rather than a total deadline — a large FLAC over
/// MTP legitimately takes minutes — this watches the byte counter and
/// gives up only when a whole [`COPY_STALL_TIMEOUT`] window passes with
/// no progress at all.
///
/// On a stall the `abort` flag is raised so the orphaned task stops at
/// its next chunk, and the caller stops waiting either way.
async fn upload_one_watched(
    transport: &SharedTransport,
    want: &Desired,
    cancel: &Arc<AtomicBool>,
    partial_name: &str,
) -> Result<u64, TransportError> {
    let progress = Arc::new(AtomicU64::new(0));
    let abort = Arc::new(AtomicBool::new(false));

    let mut task = {
        let transport = Arc::clone(transport);
        let want = want.clone();
        let cancel = Arc::clone(cancel);
        let abort = Arc::clone(&abort);
        let progress = Arc::clone(&progress);
        let partial = partial_name.to_string();
        tokio::task::spawn_blocking(move || {
            upload_one(
                transport.as_ref(),
                &want,
                &cancel,
                &abort,
                &progress,
                &partial,
            )
        })
    };

    let mut last_seen = 0u64;
    loop {
        match tokio::time::timeout(COPY_STALL_TIMEOUT, &mut task).await {
            Ok(joined) => {
                return joined.map_err(|e| TransportError::Other(anyhow::Error::from(e)))?
            }
            Err(_) => {
                let now = progress.load(Ordering::Relaxed);
                if now == last_seen {
                    abort.store(true, Ordering::Relaxed);
                    return Err(TransportError::Timeout(want.device_path.clone()));
                }
                last_seen = now;
            }
        }
    }
}

/// Stream one track onto the device, writing to a staging name and
/// renaming into place where the transport supports it, so a partial
/// transfer never occupies the real path.
fn upload_one(
    transport: &dyn DeviceTransport,
    want: &Desired,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    progress: &AtomicU64,
    partial_name: &str,
) -> Result<u64, TransportError> {
    let final_path = DevicePath::new(&want.device_path);
    let use_temp = transport.capabilities().rename;
    let write_path = match (use_temp, final_path.parent()) {
        (true, Some(parent)) => parent.join(partial_name),
        // Without rename — or at the root, which cannot happen for a
        // rendered track — write straight to the destination.
        _ => final_path.clone(),
    };

    let result = copy_one(
        transport,
        want,
        cancel,
        abort,
        progress,
        &write_path,
        use_temp,
    );
    if result.is_err() && use_temp {
        // A real transport materialises the destination on open, so a
        // failure anywhere above leaves a partial behind that no
        // manifest row would ever account for.
        let _ = transport.delete(&write_path);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn copy_one(
    transport: &dyn DeviceTransport,
    want: &Desired,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    progress: &AtomicU64,
    write_path: &DevicePath,
    use_temp: bool,
) -> Result<u64, TransportError> {
    let final_path = DevicePath::new(&want.device_path);
    let parent = final_path
        .parent()
        .ok_or_else(|| TransportError::NotFound("track has no parent directory".into()))?;
    transport.mkdir_all(&parent)?;

    let mut source = std::fs::File::open(&want.source_path)
        .map_err(|e| TransportError::NotFound(format!("{}: {e}", want.source_path)))?;
    let mut written = 0u64;
    {
        let mut sink = transport.open_write(write_path, want.size_bytes.max(0) as u64)?;
        let mut buf = vec![0u8; COPY_CHUNK];
        loop {
            // Checked every chunk, so a cancel during a large FLAC is
            // felt in about a megabyte rather than at end of file.
            // `upload_one` removes the partial.
            if cancel.load(Ordering::Relaxed) {
                return Err(TransportError::Cancelled);
            }
            // The watchdog gave up on this transfer. Unwind as soon as
            // the wedged call lets us, so the orphan does not keep
            // writing to a device the engine has moved on from.
            if abort.load(Ordering::Relaxed) {
                return Err(TransportError::Timeout(want.device_path.clone()));
            }
            let n = std::io::Read::read(&mut source, &mut buf).map_err(map_write_error)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(map_write_error)?;
            written += n as u64;
            progress.store(written, Ordering::Relaxed);
        }
        sink.flush().map_err(map_write_error)?;
    }

    if use_temp {
        if !want.replaces_existing {
            match transport.stat(&final_path)? {
                // Same size as what we were about to write. Almost
                // certainly our own file from before the manifest was
                // lost (a forgotten and re-added device, or a crash
                // between the rename and the manifest insert), so adopt
                // it instead of refusing forever with no repair path.
                Some(stat) if stat.size_bytes == want.size_bytes.max(0) as u64 => {
                    let _ = transport.delete(write_path);
                    return Ok(0);
                }
                // A different file with our name: someone else's.
                // Refusing is the whole point of the subsystem's
                // non-destruction promise.
                Some(_) => return Err(TransportError::Occupied(want.device_path.clone())),
                None => {}
            }
        }

        // Try the rename first. Where it replaces atomically the old
        // object is never absent, which matters because a failure
        // *after* an unconditional delete would leave the destination
        // empty while the manifest row still claimed the old file was
        // there — and the playlist written minutes later would point
        // at nothing.
        match transport.rename(write_path, &final_path) {
            Ok(()) => {}
            Err(first) => {
                // Not every device's rename replaces an existing name.
                // Clear and retry, but only a path we recorded writing.
                if !want.replaces_existing {
                    return Err(first);
                }
                match transport.delete(&final_path) {
                    Ok(()) | Err(TransportError::NotFound(_)) => {}
                    // Could not clear it: the destination keeps the
                    // previous good copy and the next run retries.
                    Err(e) => return Err(e),
                }
                transport.rename(write_path, &final_path).map_err(|e| {
                    TransportError::Replaced(want.device_path.clone(), e.to_string())
                })?;
            }
        }
    }
    Ok(written)
}

/// Preserve the error kind through the `io::Error` boundary.
///
/// A device filling up mid-transfer surfaces as `StorageFull`, and
/// flattening it to `Other` would let a full device report a clean sync.
fn map_write_error(e: std::io::Error) -> TransportError {
    match e.kind() {
        std::io::ErrorKind::StorageFull => TransportError::NoSpace,
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
            TransportError::Disconnected
        }
        _ => TransportError::Other(anyhow::Error::from(e)),
    }
}

/// Write one `.m3u8` per selected playlist, listing only tracks that
/// actually reached the device, and register a native playlist object
/// where the transport offers one.
async fn playlist_phase(
    engine: &SqliteRawEngine,
    transport: &SharedTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    present: &HashMap<i64, String>,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, EngineError> {
    let caps = transport.capabilities();
    let root = DevicePath::new(&device.root_path);
    let dir = root.join(PLAYLIST_DIR);

    let wanted = selected_playlists(engine, device).await?;
    let total = wanted.len() as u64;
    if total > 0 {
        let d = dir.clone();
        if let Err(e) = op(transport, "creating the playlist directory", move |t| {
            t.mkdir_all(&d).map(|_| ())
        })
        .await
        {
            // Every track already uploaded and its manifest rows are
            // committed. Failing the whole run here would throw those
            // counts away and skip `mark_synced`, so this is a warning
            // like the per-playlist write below.
            warn(
                obs,
                device.id,
                DeviceWarningKind::UploadFailed,
                format!("{}: {e}", dir.as_str()),
            );
            return Ok(0);
        }
    }

    // Paths the mirror sweep below must not touch. A playlist that is
    // still selected belongs here even when this run failed to write
    // it, or one transient error would delete the good copy already on
    // the device.
    let mut written_paths = BTreeSet::new();
    let mut count = 0u64;

    // One read, used both for the overwrite check below and by the
    // mirror sweep at the end.
    let playlist_rows: Vec<_> = device_objects::list_for_device(engine, device.id)
        .await
        .map_err(db_err)?
        .into_iter()
        .filter(|r| r.kind == KIND_PLAYLIST)
        .collect();
    // Playlist paths the manifest already claims — ours to overwrite.
    let known_paths: HashSet<&str> = playlist_rows
        .iter()
        .map(|r| r.device_path.as_str())
        .collect();

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

        // Claim the path before attempting the write, so a failure
        // below cannot let the mirror sweep delete the copy already on
        // the device for a playlist that is still selected.
        written_paths.insert(file.as_str().to_string());

        // `/Music/Playlists/<name>.m3u8` is the conventional location,
        // so the user may well have their own file there. `open_write`
        // truncates, so this must be checked first — the track path
        // makes the same check for the same reason.
        if !known_paths.contains(file.as_str()) {
            let f = file.clone();
            match op(transport, "checking a playlist path", move |t| t.stat(&f)).await {
                Ok(Some(_)) => {
                    warn(
                        obs,
                        device.id,
                        DeviceWarningKind::UploadFailed,
                        format!(
                            "{}: a file TuxTunes did not write is already there",
                            file.as_str()
                        ),
                    );
                    continue;
                }
                Ok(None) => {}
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
        }

        {
            let f = file.clone();
            let b = body.clone();
            if let Err(e) = op(transport, "writing a playlist", move |t| {
                write_text(t, &f, &b)
            })
            .await
            {
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
            let f = file.clone();
            if let Err(e) = op(transport, "registering a playlist object", move |t| {
                t.create_playlist_object(&f, &ids).map(|_| ())
            })
            .await
            {
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

        count += 1;
    }

    // Playlists we wrote on an earlier run but no longer want.
    if device.mirror_deletes && !device.key_is_weak && !cancel.load(Ordering::Relaxed) {
        for row in &playlist_rows {
            if written_paths.contains(&row.device_path) {
                continue;
            }
            let path = DevicePath::new(&row.device_path);
            match op(transport, "deleting a playlist", move |t| t.delete(&path)).await {
                // Already gone is the desired end state.
                Ok(()) | Err(TransportError::NotFound(_)) => {}
                Err(e) => {
                    // Keep the row: it is the only record that this
                    // file is ours, so dropping it would strand the
                    // file on the device forever.
                    warn(
                        obs,
                        device.id,
                        DeviceWarningKind::DeleteFailed,
                        format!("{}: {e}", row.device_path),
                    );
                    continue;
                }
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
    transport: &SharedTransport,
    obs: &dyn DeviceObserver,
    device: &DeviceRow,
    plan: &SyncPlan,
    cancel: &Arc<AtomicBool>,
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

        // Confirm the file still looks like the one we wrote before
        // removing it. A mount path is a reusable identity — swap the
        // SD card in a reader and the manifest still points at paths
        // that now hold someone else's music. This size check is the
        // guard that makes mirroring safe on such a device.
        let probe = path.clone();
        match op(transport, "checking a track before deleting", move |t| {
            t.stat(&probe)
        })
        .await
        {
            Ok(Some(stat)) if stat.size_bytes != row.size_bytes.max(0) as u64 => {
                warn(
                    obs,
                    device.id,
                    DeviceWarningKind::DeleteFailed,
                    format!(
                        "{}: left alone — the file there is not the one TuxTunes wrote",
                        row.device_path
                    ),
                );
                // Stop tracking it: whatever is there is not ours.
                device_objects::remove_by_id(engine, row.id)
                    .await
                    .map_err(db_err)?;
                continue;
            }
            // Already gone, or matches what we recorded.
            Ok(_) => {}
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

        let target = path.clone();
        match op(transport, "deleting a track", move |t| t.delete(&target)).await {
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

    let root = DevicePath::new(&device.root_path);
    op_best_effort(transport, "removing emptied directories", move |t| {
        remove_empty_dirs(t, &parents, &root)
    })
    .await;
    Ok(deleted)
}

/// Whether `path` lies strictly inside `root`, by whole segments.
///
/// String prefixing alone would treat `/Music2` as inside `/Music`.
fn is_under(path: &DevicePath, root: &DevicePath) -> bool {
    if root.as_str() == "/" {
        return path.as_str() != "/";
    }
    path.as_str()
        .strip_prefix(root.as_str())
        .is_some_and(|rest| rest.starts_with('/'))
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
        // Containment, not equality: after the user edits `root_path`,
        // old manifest paths sit outside the current root, and an
        // equality test would walk straight past it and delete
        // directories above the configured root.
        while cursor != *root && cursor.as_str() != "/" && is_under(&cursor, root) {
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

/// Remove `.tuxpart` leftovers from an interrupted earlier run.
///
/// They hold no manifest row, so nothing else would ever clean them up.
/// Only directories this run will write into are visited: walking the
/// whole device on every sync is slow over MTP, and a `.tuxpart`
/// outside the tree we are about to touch is not ours to delete.
async fn clean_partials(transport: &SharedTransport, plan: &SyncPlan, keep: &str) {
    let dirs = partial_scan_dirs(plan);
    let keep = keep.to_string();
    if dirs.is_empty() {
        return;
    }
    op_best_effort(transport, "clearing partial transfers", move |t| {
        for dir in dirs {
            let Ok(children) = t.list(&DevicePath::new(&dir)) else {
                continue;
            };
            for child in children {
                let Some(name) = child.path.file_name() else {
                    continue;
                };
                // Never this run's staging file: on a timeout this
                // sweep is orphaned rather than cancelled, and could
                // otherwise delete a transfer already in flight.
                if !child.is_dir && name.starts_with(PARTIAL_PREFIX) && name != keep {
                    let _ = t.delete(&child.path);
                }
            }
        }
    })
    .await;
}

/// The directories this run will write into or prune from.
fn partial_scan_dirs(plan: &SyncPlan) -> BTreeSet<String> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let touched = plan
        .adds
        .iter()
        .chain(plan.replaces.iter().map(|(_, d)| d))
        .map(|d| d.device_path.as_str())
        .chain(plan.orphans.iter().map(|r| r.device_path.as_str()));
    for path in touched {
        if let Some(parent) = DevicePath::new(path).parent() {
            dirs.insert(parent.as_str().to_string());
        }
    }
    dirs
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
    let by_id: HashMap<i64, &db_playlists::PlaylistRow> = all.iter().map(|p| (p.id, p)).collect();

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
