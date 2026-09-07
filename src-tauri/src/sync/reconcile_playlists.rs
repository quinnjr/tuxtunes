//! Playlist-side reconciler. Handles regular playlists, smart playlists
//! (imported as track-ID snapshots — iTunes 12+ doesn't expose smart
//! rules at the documented subtype), and folder hierarchies (two-pass
//! parent linking).

use crate::db::playlists::{self, PlaylistKind, PlaylistUpsert, PlaylistsError};
use crate::db::sync_util;
use crate::db::tracks::TracksError;
use crate::sync::events::{SyncPhase, SyncProgress, SyncWarning, WarningKind};
use crate::sync::observer::SyncObserver;
use itl_rs::ItlFile;
use prax_sqlite::raw::SqliteRawEngine;

#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
    /// Smart playlists whose iTunes criteria decoded into a live rule.
    pub smart_decoded: u64,
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

    // itl-rs ≥ 1.1 reads the folder flag from the miph header. Keep the
    // structural check as a fallback for libraries whose headers are too
    // short to carry the flag: anything another playlist calls its
    // parent is a folder regardless.
    let parent_pids: std::collections::HashSet<u64> = lib
        .playlists()
        .iter()
        .filter_map(|p| p.parent_persistent_id())
        .filter(|pid| *pid != 0)
        .collect();

    // Playlists the user deleted locally stay deleted: skip their pids
    // outright so the source can never re-import them.
    let tombstoned = playlists::tombstoned_pids(engine, source_id).await?;

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
        if tombstoned.contains(&pid) {
            continue;
        }
        keep.push(pid);

        let is_folder = p.is_folder() || parent_pids.contains(&pid);
        let (mut kind, mut smart_rule_json) = classify(p.is_smart(), is_folder);
        if kind == PlaylistKind::Smart {
            match p
                .smart_criteria()
                .map(|c| crate::sync::slst::decode(c, p.smart_info()))
            {
                Some(Ok(decoded)) => {
                    if !decoded.dropped.is_empty() {
                        stats.warnings += 1;
                        obs.warning(&SyncWarning {
                            source_id,
                            kind: WarningKind::SmartRulePartial,
                            detail: format!(
                                "{:?}: dropped rules TuxTunes cannot evaluate: {}",
                                p.title(),
                                decoded.dropped.join(", ")
                            ),
                        });
                    }
                    smart_rule_json = serde_json::to_string(&decoded.rule).ok();
                    stats.smart_decoded += 1;
                }
                Some(Err(e)) => {
                    // Keep the snapshot iTunes resolved as a static list.
                    stats.warnings += 1;
                    obs.warning(&SyncWarning {
                        source_id,
                        kind: WarningKind::SmartRuleDecodeFailed,
                        detail: format!("{:?}: {e}; imported as a static playlist", p.title()),
                    });
                    kind = PlaylistKind::Regular;
                }
                None => kind = PlaylistKind::Regular,
            }
        }

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

        match p.parent_persistent_id() {
            Some(parent_pid) => pending_parent_links.push((local_id, parent_pid)),
            // The source no longer reports a parent for a playlist we
            // already knew about — clear the stale link rather than
            // leaving it pointing at whatever it last resolved to. A
            // freshly-inserted row has no parent to clear.
            None if existed => playlists::link_parent(engine, local_id, None).await?,
            None => {}
        }
    }

    let playlist_pid_to_local = sync_util::load_pid_to_local_id_map(engine, "playlists", source_id)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    // Current parent_id for every playlist, kept up to date locally as
    // links are applied below, so each candidate link can be checked
    // for a cycle against the state it would actually land in.
    let mut parent_of = playlists::parent_id_map(engine).await?;
    for (child_id, parent_pid) in pending_parent_links {
        let parent_local = playlist_pid_to_local.get(&parent_pid).copied();
        match parent_local {
            Some(parent_local) if would_create_cycle(&parent_of, child_id, parent_local) => {
                stats.warnings += 1;
                log::warn!(
                    "sync: playlist {child_id}'s parent link to {parent_local} would create \
                     a cycle; keeping its existing parent"
                );
                continue;
            }
            _ => {}
        }
        playlists::link_parent(engine, child_id, parent_local).await?;
        parent_of.insert(child_id, parent_local);
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

/// True if setting `child`'s parent to `new_parent` would create a
/// `parent_id` cycle, given `parent_of` — the current (or
/// already-applied-this-pass) parent link for every local playlist id.
/// Walks `new_parent`'s own parent chain looking for `child`; a source
/// library can legitimately contain a cycle (a folder moved under its
/// own descendant since the last sync), so this is a check the caller
/// consults before committing the link, not a panic condition.
fn would_create_cycle(
    parent_of: &std::collections::HashMap<i64, Option<i64>>,
    child: i64,
    new_parent: i64,
) -> bool {
    let mut cursor = Some(new_parent);
    let mut hops = 0;
    while let Some(id) = cursor {
        if id == child {
            return true;
        }
        hops += 1;
        if hops > parent_of.len() {
            // Data already inconsistent (a dangling/cyclic chain not
            // rooted in `parent_of`); don't spin forever over it.
            return false;
        }
        cursor = parent_of.get(&id).copied().flatten();
    }
    false
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

    #[test]
    fn would_create_cycle_detects_direct_and_indirect_cycles() {
        // 2 -> 1 already linked; proposing 1's parent = 2 closes the loop.
        let parent_of = std::collections::HashMap::from([(1, None), (2, Some(1))]);
        assert!(would_create_cycle(&parent_of, 1, 2));

        // 3 -> 2 -> 1; proposing 1's parent = 3 closes a longer loop.
        let parent_of = std::collections::HashMap::from([(1, None), (2, Some(1)), (3, Some(2))]);
        assert!(would_create_cycle(&parent_of, 1, 3));

        // Same shape, but proposing an unrelated node's parent is fine.
        assert!(!would_create_cycle(&parent_of, 4, 3));
    }

    #[test]
    fn would_create_cycle_false_for_acyclic_chain() {
        let parent_of = std::collections::HashMap::from([(1, None), (2, Some(1))]);
        assert!(!would_create_cycle(&parent_of, 3, 2));
    }
}
