//! Copy-on-add / copy-on-sync worker.
//!
//! Per-file flow:
//!   1. Hash source for verification.
//!   2. Render target path from `organize_scheme` + track row.
//!   3. If the source already *is* that path, skip to step 7.
//!   4. Resolve filename collisions (suffix mode), against both the
//!      filesystem and the `file_path` column.
//!   5. Copy source → target, then re-hash it and verify the match
//!      (delete + error otherwise).
//!   6. Extract artwork alongside.
//!   7. Write `file_path`, `original_path`, `file_hash`, `artwork_path`
//!      to the DB — removing the copy again if the row is gone.
//!
//! Every failure emits `fs:ingest-failed`. Only one whose source has
//! also vanished marks `import_status = 'missing_source'`: a copy that
//! failed on its own (a full or read-only library root) leaves a row
//! that still plays from where it is.

use crate::db::preferences;
use crate::db::tracks::{self, TrackRow};
use crate::fs::artwork;
use crate::fs::events::{IngestComplete, IngestFailed, INGEST_COMPLETE, INGEST_FAILED};
use crate::fs::hash;
use crate::fs::path::{self, render, TrackFields};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum IngestCommand {
    CopyForTrack { track_id: i64, source_path: PathBuf },
}

pub struct IngestWorker {
    pub tx: mpsc::UnboundedSender<IngestCommand>,
    _task: tokio::task::JoinHandle<()>,
}

impl IngestWorker {
    pub fn spawn<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<IngestCommand>();
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    IngestCommand::CopyForTrack {
                        track_id,
                        source_path,
                    } => {
                        if let Err(e) = ingest_one(&engine, &app, track_id, &source_path).await {
                            log::warn!(
                                "ingest failed for track {track_id} ({}): {e}",
                                source_path.display()
                            );
                            let _ = app.emit(
                                INGEST_FAILED,
                                IngestFailed {
                                    track_id,
                                    source_path: source_path.display().to_string(),
                                    error: e.to_string(),
                                },
                            );
                            // Only a source that is actually gone makes
                            // the track unplayable. A copy that failed
                            // for any other reason (full disk, a
                            // read-only library root) leaves the row
                            // pointing at a file that still plays
                            // fine — marking it missing would grey it
                            // out and drop it from auto-advance.
                            if !source_path.exists() {
                                let _ = tracks::mark_missing_source(&engine, track_id).await;
                            }
                        }
                    }
                }
            }
        });
        Self { tx, _task: task }
    }
}

async fn ingest_one<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
    track_id: i64,
    source_path: &std::path::Path,
) -> anyhow::Result<()> {
    // Per-track progress emits are intentionally omitted — the
    // fs:ingest-complete event fires once per track and carries the
    // managed path, so the UI can track progress without the IPC
    // overhead of per-stage chatter (~150K events on a 51K-track sync).
    let source_hash = tokio::task::spawn_blocking({
        let p = source_path.to_path_buf();
        move || hash::hash_file(&p)
    })
    .await??;

    let row: TrackRow = tracks::get(engine, track_id).await?;
    let root = preferences::get_library_root(engine).await?;
    let scheme = preferences::get_organize_scheme(engine).await?;

    let rel = render(&scheme, &TrackFields::from_track_row(&row, source_path))?;
    let ideal = root.join(&rel);

    // A file already sitting at the exact path the scheme asks for
    // needs no copy — copying would just write a `(2)` duplicate beside
    // it. Compared through `same_file` because the source comes from a
    // file picker and the root from a preference, so the two often
    // spell the same location differently (symlinked $HOME, `..`).
    // Anything else under the root — a drop folder, an unorganised
    // stash — is still copied into place, exactly like an outside file.
    let already_managed = same_file(&ideal, source_path);

    let (target_abs, target_hash) = if already_managed {
        (source_path.to_path_buf(), source_hash)
    } else {
        copy_into_library(engine, source_path, &ideal, source_hash).await?
    };

    let artwork_result = tokio::task::spawn_blocking({
        let p = target_abs.clone();
        move || artwork::extract_cover_alongside(&p)
    })
    .await?;
    // Non-audio files (tests use 1 KB of 0xAB) cause Lofty to bail;
    // treat as "no artwork" rather than failing the whole ingest.
    let artwork = artwork_result.unwrap_or(None);

    let artwork_str = artwork.as_ref().map(|p| p.display().to_string());

    // `original_path` records where a copied file came from. For a file
    // that was already in place there is no separate original, and
    // passing None leaves any value a previous sync recorded alone.
    let original = (!already_managed).then(|| source_path.display().to_string());

    let updated = tracks::set_file_paths(
        engine,
        track_id,
        &target_abs.display().to_string(),
        original.as_deref(),
        &hash::hash_hex(target_hash),
        artwork_str.as_deref(),
    )
    .await?;

    if updated == 0 {
        // The row was removed while the copy was in flight. Nothing
        // will ever reference the file we just wrote, so take it back
        // out rather than leaving litter under the managed root.
        if !already_managed {
            let _ = std::fs::remove_file(&target_abs);
        }
        anyhow::bail!("track {track_id} disappeared before its copy landed");
    }

    let _ = app.emit(
        INGEST_COMPLETE,
        IngestComplete {
            track_id,
            managed_path: target_abs.display().to_string(),
            artwork_path: artwork_str,
        },
    );
    Ok(())
}

/// Copy `source_path` to the first free name at or beside `ideal`,
/// verify the bytes landed intact, and return the final path with its
/// hash. Any failure after the copy starts removes the partial target.
async fn copy_into_library(
    engine: &SqliteRawEngine,
    source_path: &std::path::Path,
    ideal: &std::path::Path,
    source_hash: u64,
) -> anyhow::Result<(PathBuf, u64)> {
    let target_abs = free_target(engine, ideal).await?;

    if let Some(parent) = target_abs.parent() {
        let parent = parent.to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(parent)).await??;
    }

    let copied = tokio::task::spawn_blocking({
        let src = source_path.to_path_buf();
        let dst = target_abs.clone();
        move || std::fs::copy(&src, &dst)
    })
    .await?;
    if let Err(e) = copied {
        // std::fs::copy leaves whatever it managed to write behind — a
        // truncated file no row will ever own, holding the name a retry
        // wants.
        let _ = std::fs::remove_file(&target_abs);
        return Err(e.into());
    }

    // std::fs::copy carries the source's mode across, so a file from a
    // read-only mount would land unwritable and every later tag edit
    // would fail. The managed copy is ours to rewrite.
    make_owner_writable(&target_abs);

    let target_hash = tokio::task::spawn_blocking({
        let p = target_abs.clone();
        move || hash::hash_file(&p)
    })
    .await??;
    if target_hash != source_hash {
        // The source may simply have changed under us mid-copy (a tag
        // edit on a track whose copy is still queued). Re-read it: if
        // it now matches what we wrote, the copy is a faithful snapshot
        // and only the pre-copy hash was stale.
        let recheck = tokio::task::spawn_blocking({
            let p = source_path.to_path_buf();
            move || hash::hash_file(&p)
        })
        .await?;
        if recheck.ok() != Some(target_hash) {
            let _ = std::fs::remove_file(&target_abs);
            anyhow::bail!("copy hash mismatch — target deleted");
        }
    }

    Ok((target_abs, target_hash))
}

/// First candidate name at or beside `ideal` that is free both on disk
/// and in the `tracks` table. `file_path` is UNIQUE, so a name another
/// row still claims — typically one whose file the user deleted behind
/// the app's back — would fail the write *after* the copy landed.
async fn free_target(engine: &SqliteRawEngine, ideal: &std::path::Path) -> anyhow::Result<PathBuf> {
    for n in 0..=path::COLLISION_ATTEMPTS {
        let cand = path::collision_candidate(ideal, n);
        if !cand.exists() && !tracks::path_in_use(engine, &cand.display().to_string()).await? {
            return Ok(cand);
        }
    }
    Ok(path::collision_candidate(
        ideal,
        path::COLLISION_ATTEMPTS + 1,
    ))
}

/// Whether two paths name the same file. Falls back to comparing the
/// paths as written when either side does not exist yet — which is the
/// normal case for the copy target.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Add the owner-write bit if it is missing. Best-effort: a filesystem
/// without Unix permissions (or a failed chmod) is not worth failing an
/// otherwise good copy over.
fn make_owner_writable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let mut perms = meta.permissions();
        let mode = perms.mode();
        if mode & 0o200 == 0 {
            perms.set_mode(mode | 0o200);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}
