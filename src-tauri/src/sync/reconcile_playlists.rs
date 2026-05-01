//! Playlist-side reconciler. Handles regular playlists, smart playlists
//! (imported as track-ID snapshots — iTunes 12+ doesn't expose smart
//! rules at the documented subtype), and folder hierarchies (two-pass
//! parent linking).

use crate::db::playlists::{self, PlaylistKind, PlaylistUpsert, PlaylistsError};
use crate::db::sync_util::{self, pid_hex};
use crate::db::tracks::TracksError;
use crate::sync::events::{SyncPhase, SyncProgress};
use itl_rs::ItlFile;
use prax_sqlite::raw::SqliteRawEngine;
use tauri::{AppHandle, Emitter, Runtime};

#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
}

pub async fn reconcile<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
    source_id: i64,
    lib: &ItlFile,
) -> Result<PlaylistReconcileStats, PlaylistsError> {
    let mut stats = PlaylistReconcileStats::default();
    let total = lib.playlists().len() as u64;

    // ITL internal track id (u32) → persistent_id (u64) — entirely
    // derived from `lib` without touching SQLite.
    let mut itl_to_pid: std::collections::HashMap<u32, u64> =
        std::collections::HashMap::with_capacity(lib.tracks().len());
    for t in lib.tracks() {
        itl_to_pid.insert(t.id(), t.persistent_id());
    }

    let track_pid_to_local = sync_util::load_pid_to_local_id_map(engine, "tracks", source_id)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;

    let mut keep: Vec<u64> = Vec::with_capacity(lib.playlists().len());
    let mut pending_parent_links: Vec<(i64, u64)> = Vec::new();

    for (idx, p) in lib.playlists().iter().enumerate() {
        if idx % 50 == 0 {
            let _ = app.emit(
                crate::sync::events::PROGRESS,
                SyncProgress {
                    source_id,
                    phase: SyncPhase::ApplyingPlaylists,
                    current: idx as u64,
                    total,
                    message: format!("{idx} / {total}"),
                },
            );
        }

        let pid = p.persistent_id();
        if pid == 0 {
            stats.warnings += 1;
            continue;
        }
        keep.push(pid);

        let (kind, smart_rule_json) = classify(p);

        // Translate ITL track IDs to local row IDs; skip any track we
        // didn't import (zero pid, unmappable path, etc.).
        let track_entries: Vec<i64> = p
            .track_ids()
            .iter()
            .filter_map(|itl_id| {
                let track_pid = itl_to_pid.get(itl_id)?;
                track_pid_to_local.get(track_pid).copied()
            })
            .collect();

        let upsert = PlaylistUpsert {
            persistent_id: pid,
            sync_source_id: source_id,
            name: p.title().unwrap_or("<untitled>"),
            kind,
            parent_persistent_id: p.parent_persistent_id(),
            sort_order: idx as i64,
            track_entries: &track_entries,
            smart_rule_json,
        };

        let existed = playlists::by_persistent_id(engine, source_id, &pid_hex(pid))
            .await?
            .is_some();
        let local_id = playlists::upsert(engine, &upsert).await?;
        if existed {
            stats.updated += 1;
        } else {
            stats.inserted += 1;
        }

        if let Some(parent_pid) = p.parent_persistent_id() {
            pending_parent_links.push((local_id, parent_pid));
        }
    }

    let playlist_pid_to_local = sync_util::load_pid_to_local_id_map(engine, "playlists", source_id)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    for (child_id, parent_pid) in pending_parent_links {
        let parent_local = playlist_pid_to_local.get(&parent_pid).copied();
        playlists::link_parent(engine, child_id, parent_local).await?;
    }

    let deleted = playlists::delete_missing(engine, source_id, &keep).await?;
    stats.deleted = deleted;

    Ok(stats)
}

fn classify(p: &itl_rs::Playlist) -> (PlaylistKind, Option<String>) {
    if p.is_folder() {
        (PlaylistKind::Folder, None)
    } else if p.is_smart() {
        (PlaylistKind::Smart, None)
    } else {
        (PlaylistKind::Regular, None)
    }
}

impl From<TracksError> for PlaylistsError {
    fn from(e: TracksError) -> Self {
        PlaylistsError::Query(anyhow::Error::msg(e.to_string()))
    }
}
