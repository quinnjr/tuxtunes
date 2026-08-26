//! Single-task sync worker. Reads `SyncCommand`s from an unbounded
//! channel, runs one reconcile at a time.

use crate::db::{sync_sources, Db};
use crate::fs::coordinator::FsCoordinator;
use crate::sync::events::{
    SyncComplete, SyncFailed, SyncPhase, SyncProgress, SyncWarning, WarningKind,
};
use crate::sync::import_log::{ImportLog, LogLevel, LogSink};
use crate::sync::log_tailer::LogTailer;
use crate::sync::observer::{SyncObserver, TauriObserver};
use crate::sync::{reconcile_playlists, reconcile_tracks};
use itl_rs::ItlFile;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
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
    pub fn spawn<R: Runtime>(db: Arc<Db>, fs: Arc<FsCoordinator>, app: AppHandle<R>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
        let db_clone = Arc::clone(&db);
        let fs_clone = Arc::clone(&fs);
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    SyncCommand::RunNow { source_id } => {
                        run_one(&db_clone, &fs_clone, &app, source_id).await;
                    }
                }
            }
        });
        Self { tx, _task: task }
    }
}

/// GUI run: set up the per-run log file + live tailer (`sync:log`), build a
/// `TauriObserver`, drive the reconcile, and emit the terminal
/// complete/failed event. The CLI bypasses this and calls
/// [`reconcile_source`] directly with its own observer and log sink.
async fn run_one<R: Runtime>(
    db: &Arc<Db>,
    fs: &Arc<FsCoordinator>,
    app: &AppHandle<R>,
    source_id: i64,
) {
    let obs = TauriObserver::new(app.clone());

    // Best-effort per-run log file. A missing log must never fail the import.
    let import_log = match ImportLog::create(app, source_id) {
        Ok(l) => Some(l),
        Err(e) => {
            log::warn!("import log unavailable: {e}");
            None
        }
    };

    // Live tailer: stream each new log line to the UI as `sync:log`.
    let tailer = import_log.as_ref().map(|l| {
        let app = app.clone();
        LogTailer::spawn(l.path().to_path_buf(), move |seq, line| {
            let _ = app.emit(
                crate::sync::events::LOG,
                crate::sync::events::LogLine {
                    source_id,
                    seq,
                    line,
                },
            );
        })
    });

    let sink = |level: LogLevel, msg: &str| {
        if let Some(l) = &import_log {
            l.write(level, msg);
        }
    };

    match reconcile_source(db, Some(fs), &obs, &sink, source_id).await {
        Ok(complete) => obs.complete(&complete),
        Err(e) => {
            sink(LogLevel::Warn, &format!("failed: {e}"));
            obs.failed(&SyncFailed {
                source_id,
                error: e.to_string(),
            });
        }
    }

    // Stop after a final drain so the closing lines reach the UI.
    if let Some(t) = tailer {
        t.stop().await;
    }
}

/// Run one full reconcile for `source_id`: parse the .itl, apply tracks and
/// playlists, optionally ingest files (only when `fs` is `Some` and the source
/// opts in), and finalize. Progress/warning events go to `obs`; human-readable
/// lines go to `log_sink`. Returns the completion summary — emitting the
/// terminal `complete`/`failed` event is the caller's job.
pub async fn reconcile_source(
    db: &Arc<Db>,
    fs: Option<&FsCoordinator>,
    obs: &dyn SyncObserver,
    log_sink: LogSink<'_>,
    source_id: i64,
) -> Result<SyncComplete, anyhow::Error> {
    let source = sync_sources::get(&db.engine, source_id).await?;

    log_sink(LogLevel::Info, "reading .itl");
    obs.progress(&SyncProgress {
        source_id,
        phase: SyncPhase::Decoding,
        current: 0,
        total: 0,
        message: "reading .itl".into(),
    });
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

    obs.progress(&SyncProgress {
        source_id,
        phase: SyncPhase::ApplyingTracks,
        current: 0,
        total: lib.tracks().len() as u64,
        message: "applying tracks".into(),
    });
    log_sink(
        LogLevel::Info,
        &format!("applying tracks: {}", lib.tracks().len()),
    );

    let reconcile_tracks::TrackReconcileOutcome {
        stats: track_stats,
        ingest_candidates,
        aliases,
    } = reconcile_tracks::reconcile(
        &db.engine,
        obs,
        source_id,
        &lib,
        &source.path_mappings,
        &source.conflict_rules,
        log_sink,
    )
    .await?;

    if source.auto_copy_files {
        match fs {
            Some(fs) => {
                for cand in ingest_candidates {
                    let _ = fs.copy_for_track(cand.track_id, cand.source_path);
                }
            }
            None => {
                obs.warning(&SyncWarning {
                    source_id,
                    kind: WarningKind::IngestSkipped,
                    detail: "auto_copy_files is on but file ingest is GUI-only; skipping copy"
                        .into(),
                });
                log_sink(LogLevel::Warn, "file ingest is GUI-only; skipping copy");
            }
        }
    }

    obs.progress(&SyncProgress {
        source_id,
        phase: SyncPhase::ApplyingPlaylists,
        current: 0,
        total: lib.playlists().len() as u64,
        message: "applying playlists".into(),
    });
    log_sink(
        LogLevel::Info,
        &format!("applying playlists: {}", lib.playlists().len()),
    );

    let pl_stats =
        reconcile_playlists::reconcile(&db.engine, obs, source_id, &lib, &aliases).await?;

    obs.progress(&SyncProgress {
        source_id,
        phase: SyncPhase::Finalizing,
        current: 0,
        total: 0,
        message: "finalizing".into(),
    });
    log_sink(LogLevel::Info, "finalizing");
    sync_sources::finalize_sync(&db.engine, source_id, &hash).await?;

    let complete = SyncComplete {
        source_id,
        inserted_tracks: track_stats.inserted,
        updated_tracks: track_stats.updated,
        deleted_tracks: track_stats.deleted,
        inserted_playlists: pl_stats.inserted,
        updated_playlists: pl_stats.updated,
        deleted_playlists: pl_stats.deleted,
    };
    log_sink(
        LogLevel::Info,
        &format!(
            "done — tracks +{}/~{}/-{}, playlists +{}/~{}/-{}",
            complete.inserted_tracks,
            complete.updated_tracks,
            complete.deleted_tracks,
            complete.inserted_playlists,
            complete.updated_playlists,
            complete.deleted_playlists,
        ),
    );
    Ok(complete)
}
