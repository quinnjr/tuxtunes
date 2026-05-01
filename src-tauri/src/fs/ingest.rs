//! Copy-on-add / copy-on-sync worker.
//!
//! Per-file flow:
//!   1. Hash source for verification.
//!   2. Render target path from `organize_scheme` + track row.
//!   3. Resolve filename collisions (suffix mode).
//!   4. Copy source → target.
//!   5. Re-hash target, verify match (delete + error on mismatch).
//!   6. Extract artwork alongside.
//!   7. Write `file_path`, `original_path`, `file_hash`, `artwork_path`
//!      to the DB.
//!
//! On any failure, mark `import_status = 'missing_source'` via
//! `tracks::mark_missing_source` and emit `fs:ingest-failed`.

use crate::db::preferences;
use crate::db::tracks::{self, TrackRow};
use crate::fs::artwork;
use crate::fs::events::{
    IngestComplete, IngestFailed, IngestProgress, INGEST_COMPLETE, INGEST_FAILED, INGEST_PROGRESS,
};
use crate::fs::hash;
use crate::fs::path::{render, resolve_collision, TrackFields};
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
                            let _ = app.emit(
                                INGEST_FAILED,
                                IngestFailed {
                                    track_id,
                                    source_path: source_path.display().to_string(),
                                    error: e.to_string(),
                                },
                            );
                            let _ = tracks::mark_missing_source(&engine, track_id).await;
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
    let _ = app.emit(
        INGEST_PROGRESS,
        IngestProgress {
            track_id,
            current: 0,
            total: 0,
            message: "hashing source".into(),
        },
    );
    let source_hash = tokio::task::spawn_blocking({
        let p = source_path.to_path_buf();
        move || hash::hash_file(&p)
    })
    .await??;

    let row: TrackRow = tracks::get(engine, track_id).await?;
    let root = preferences::get_library_root(engine).await?;
    let scheme = preferences::get_organize_scheme(engine).await?;

    let ext = source_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let fallback_stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let rel = render(
        &scheme,
        &TrackFields {
            title: &row.title,
            artist: row.artist.as_deref(),
            album_artist: None,
            album: row.album.as_deref(),
            genre: None,
            track_number: None,
            track_count: None,
            disc_number: None,
            disc_count: None,
            year: None,
            ext: &ext,
            fallback_stem: &fallback_stem,
        },
    )?;

    let target_abs = resolve_collision(&root.join(&rel));
    if let Some(parent) = target_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let _ = app.emit(
        INGEST_PROGRESS,
        IngestProgress {
            track_id,
            current: 0,
            total: 0,
            message: format!("copying to {}", target_abs.display()),
        },
    );
    tokio::task::spawn_blocking({
        let src = source_path.to_path_buf();
        let dst = target_abs.clone();
        move || std::fs::copy(&src, &dst)
    })
    .await??;

    let target_hash = tokio::task::spawn_blocking({
        let p = target_abs.clone();
        move || hash::hash_file(&p)
    })
    .await??;
    if target_hash != source_hash {
        let _ = std::fs::remove_file(&target_abs);
        anyhow::bail!("copy hash mismatch — target deleted");
    }

    let artwork_result = tokio::task::spawn_blocking({
        let p = target_abs.clone();
        move || artwork::extract_cover_alongside(&p)
    })
    .await?;
    // Non-audio files (tests use 1 KB of 0xAB) cause Lofty to bail;
    // treat as "no artwork" rather than failing the whole ingest.
    let artwork = artwork_result.unwrap_or(None);

    let artwork_str = artwork.as_ref().map(|p| p.display().to_string());

    tracks::set_file_paths(
        engine,
        track_id,
        &target_abs.display().to_string(),
        Some(&source_path.display().to_string()),
        &hash::hash_hex(target_hash),
        artwork_str.as_deref(),
    )
    .await?;

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
