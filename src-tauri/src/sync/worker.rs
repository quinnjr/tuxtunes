//! Single-task sync worker. Reads `SyncCommand`s from an unbounded
//! channel, runs one reconcile at a time.

use crate::db::{sync_sources, Db};
use crate::fs::coordinator::FsCoordinator;
use crate::sync::events::{SyncComplete, SyncFailed, SyncPhase, SyncProgress};
use crate::sync::{reconcile_playlists, reconcile_tracks};
use itl_rs::ItlFile;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum SyncCommand {
    RunNow { source_id: i64 },
}

pub struct SyncWorker {
    pub tx: mpsc::UnboundedSender<SyncCommand>,
    _task: tokio::task::JoinHandle<()>,
}

impl SyncWorker {
    pub fn spawn(db: Arc<Db>, fs: Arc<FsCoordinator>, app: AppHandle) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
        let db_clone = Arc::clone(&db);
        let fs_clone = Arc::clone(&fs);
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    SyncCommand::RunNow { source_id } => {
                        if let Err(e) = run_one(&db_clone, &fs_clone, &app, source_id).await {
                            let _ = app.emit(
                                crate::sync::events::FAILED,
                                SyncFailed {
                                    source_id,
                                    error: e.to_string(),
                                },
                            );
                        }
                    }
                }
            }
        });
        Self { tx, _task: task }
    }
}

async fn run_one(
    db: &Arc<Db>,
    fs: &Arc<FsCoordinator>,
    app: &AppHandle,
    source_id: i64,
) -> Result<(), anyhow::Error> {
    let source = sync_sources::get(&db.engine, source_id).await?;

    let _ = app.emit(
        crate::sync::events::PROGRESS,
        SyncProgress {
            source_id,
            phase: SyncPhase::Decoding,
            current: 0,
            total: 0,
            message: "reading .itl".into(),
        },
    );
    let lib = ItlFile::open(&source.source_path)?;

    // Quick hash of the file size + mtime for last_sync_hash. A real
    // content hash would take too long on 17 MB files called every sync.
    let meta = std::fs::metadata(&source.source_path)?;
    let hash = format!(
        "{}:{}",
        meta.len(),
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );

    let _ = app.emit(
        crate::sync::events::PROGRESS,
        SyncProgress {
            source_id,
            phase: SyncPhase::ApplyingTracks,
            current: 0,
            total: lib.tracks().len() as u64,
            message: "applying tracks".into(),
        },
    );

    let (track_stats, ingest_candidates) = reconcile_tracks::reconcile(
        &db.engine,
        app,
        source_id,
        &lib,
        &source.path_mappings,
        &source.conflict_rules,
    )
    .await?;

    if source.auto_copy_files {
        for cand in ingest_candidates {
            let _ = fs.copy_for_track(cand.track_id, cand.source_path);
        }
    }

    let _ = app.emit(
        crate::sync::events::PROGRESS,
        SyncProgress {
            source_id,
            phase: SyncPhase::ApplyingPlaylists,
            current: 0,
            total: lib.playlists().len() as u64,
            message: "applying playlists".into(),
        },
    );

    let pl_stats = reconcile_playlists::reconcile(&db.engine, app, source_id, &lib).await?;

    let _ = app.emit(
        crate::sync::events::PROGRESS,
        SyncProgress {
            source_id,
            phase: SyncPhase::Finalizing,
            current: 0,
            total: 0,
            message: "finalizing".into(),
        },
    );
    sync_sources::finalize_sync(&db.engine, source_id, &hash).await?;

    let _ = app.emit(
        crate::sync::events::COMPLETE,
        SyncComplete {
            source_id,
            inserted_tracks: track_stats.inserted,
            updated_tracks: track_stats.updated,
            deleted_tracks: track_stats.deleted,
            inserted_playlists: pl_stats.inserted,
            updated_playlists: pl_stats.updated,
            deleted_playlists: pl_stats.deleted,
        },
    );

    Ok(())
}
