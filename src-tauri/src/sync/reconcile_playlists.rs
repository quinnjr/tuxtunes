//! Playlist-side reconciler. Handles regular playlists, smart playlists
//! (imported as track-ID snapshots — iTunes 12+ doesn't expose smart
//! rules at the documented subtype), and folder hierarchies (two-pass
//! parent linking).

use crate::db::playlists::{self, PlaylistKind, PlaylistUpsert, PlaylistsError};
use crate::db::sync_util;
use crate::db::tracks::TracksError;
use crate::sync::events::{SyncPhase, SyncProgress};
use crate::sync::observer::SyncObserver;
use itl_rs::ItlFile;
use prax_sqlite::raw::SqliteRawEngine;

#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
}

pub async fn reconcile(
    engine: &SqliteRawEngine,
    obs: &dyn SyncObserver,
    source_id: i64,
    lib: &ItlFile,
    aliases: &std::collections::HashMap<u64, u64>,
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

    // Pre-resolved pid → local-id map for playlists that already exist,
    // batch-loaded once instead of a `by_persistent_id` SELECT per row.
    // Reloaded after the loop (as `playlist_pid_to_local` below) so that
    // newly-inserted playlists are visible for parent-link resolution.
    let playlist_pid_to_local_initial =
        sync_util::load_pid_to_local_id_map(engine, "playlists", source_id)
            .await
            .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;

    // Folder-ness is derived from structure, not from itl-rs's
    // `is_folder()` heuristic ("has no tracks"): iTunes folders carry
    // the union of their children's tracks, and genuinely empty
    // playlists exist, so that heuristic inverts both cases.
    let parent_pids: std::collections::HashSet<u64> = lib
        .playlists()
        .iter()
        .filter_map(|p| p.parent_persistent_id())
        .filter(|pid| *pid != 0)
        .collect();

    let mut keep: Vec<u64> = Vec::with_capacity(lib.playlists().len());
    let mut pending_parent_links: Vec<(i64, u64)> = Vec::new();

    for (idx, p) in lib.playlists().iter().enumerate() {
        if idx % 50 == 0 {
            obs.progress(&SyncProgress {
                source_id,
                phase: SyncPhase::ApplyingPlaylists,
                current: idx as u64,
                total,
                message: format!("{idx} / {total}"),
            });
        }

        let pid = p.persistent_id();
        if pid == 0 {
            stats.warnings += 1;
            continue;
        }
        keep.push(pid);

        let (kind, smart_rule_json) = classify(p.is_smart(), parent_pids.contains(&pid));

        // Translate ITL track IDs to local row IDs; skip any track we
        // didn't import (zero pid, unmappable path, etc.).
        // A folder's own entry list is the union of its children —
        // never something to show as a playlist of its own.
        let track_entries: Vec<i64> = if kind == PlaylistKind::Folder {
            Vec::new()
        } else {
            p.track_ids()
                .iter()
                .filter_map(|itl_id| {
                    let track_pid = itl_to_pid.get(itl_id)?;
                    // A track merged into another (same file) keeps its
                    // playlist slots via the survivor.
                    let track_pid = aliases.get(track_pid).unwrap_or(track_pid);
                    track_pid_to_local.get(track_pid).copied()
                })
                .collect()
        };

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

        let known_local_id = playlist_pid_to_local_initial.get(&pid).copied();
        let existed = known_local_id.is_some();
        let local_id = playlists::upsert_with_known_id(engine, &upsert, known_local_id).await?;
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

/// `has_children` wins: anything another playlist calls its parent is
/// a folder whatever its own flags or track count say. Smart detection
/// is itl-rs's (unreliable on modern libraries, but harmless: a smart
/// playlist mis-typed as regular still lists its resolved tracks).
fn classify(is_smart: bool, has_children: bool) -> (PlaylistKind, Option<String>) {
    if has_children {
        (PlaylistKind::Folder, None)
    } else if is_smart {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_prefers_structure_over_flags() {
        assert_eq!(classify(false, true).0, PlaylistKind::Folder);
        assert_eq!(classify(true, true).0, PlaylistKind::Folder);
        assert_eq!(classify(true, false).0, PlaylistKind::Smart);
        assert_eq!(classify(false, false).0, PlaylistKind::Regular);
    }
}
