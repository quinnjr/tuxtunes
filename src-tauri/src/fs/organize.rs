//! Rename managed-library files when metadata changes. Called by the UI
//! metadata-edit flow (gated on `keep_organized`) and by bulk
//! "re-organize library" actions.

use crate::db::preferences;
use crate::db::tracks::{self, TrackRow};
use crate::fs::events::{OrganizeApplied, ORGANIZE_APPLIED};
use crate::fs::path::{render, resolve_collision, TrackFields};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum OrganizeCommand {
    ReorganizeTrack { track_id: i64 },
}

pub struct OrganizeWorker {
    pub tx: mpsc::UnboundedSender<OrganizeCommand>,
    _task: tokio::task::JoinHandle<()>,
}

impl OrganizeWorker {
    pub fn spawn<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<OrganizeCommand>();
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    OrganizeCommand::ReorganizeTrack { track_id } => {
                        if let Err(e) = organize_one(&engine, &app, track_id).await {
                            eprintln!("organize failed for track {track_id}: {e}");
                        }
                    }
                }
            }
        });
        Self { tx, _task: task }
    }
}

async fn organize_one<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
    track_id: i64,
) -> anyhow::Result<()> {
    let row: TrackRow = tracks::get(engine, track_id).await?;
    let root = preferences::get_library_root(engine).await?;
    let scheme = preferences::get_organize_scheme(engine).await?;

    let old_path = PathBuf::from(&row.file_path);
    if !old_path.exists() {
        anyhow::bail!("source file missing at {}", old_path.display());
    }
    let ext = old_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let fallback_stem = old_path
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
    let new_abs = root.join(&rel);
    if new_abs == old_path {
        return Ok(());
    }

    if let Some(parent) = new_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Suffix mode (matches ingest). Future work: add an `error` mode
    // per the v1 design for user-initiated edits.
    let new_abs = resolve_collision(&new_abs);
    std::fs::rename(&old_path, &new_abs)?;

    // Prune now-empty parent dirs up to library_root.
    if let Some(parent) = old_path.parent() {
        let mut cur = Some(parent.to_path_buf());
        while let Some(p) = cur {
            if p == root || !p.starts_with(&root) {
                break;
            }
            match std::fs::read_dir(&p) {
                Ok(mut it) => {
                    if it.next().is_none() {
                        let _ = std::fs::remove_dir(&p);
                        cur = p.parent().map(std::path::Path::to_path_buf);
                    } else {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    let sql = "UPDATE tracks SET file_path = ? WHERE id = ?";
    use prax_query::filter::FilterValue as FV;
    engine
        .raw_sql_execute(
            sql,
            &[FV::String(new_abs.display().to_string()), FV::Int(track_id)],
        )
        .await?;

    let _ = app.emit(
        ORGANIZE_APPLIED,
        OrganizeApplied {
            track_id,
            old_path: old_path.display().to_string(),
            new_path: new_abs.display().to_string(),
        },
    );
    Ok(())
}
