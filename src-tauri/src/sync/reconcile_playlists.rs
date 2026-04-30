//! Playlist-side reconciler. Handles regular playlists, smart playlists
//! (decoding SmartRule via itl-rs), and folder hierarchies (two-pass
//! parent linking).

use crate::db::playlists::{self, PlaylistKind, PlaylistUpsert, PlaylistsError};
use crate::db::tracks::TracksError;
use crate::sync::events::{SyncPhase, SyncProgress};
use itl_rs::ItlFile;
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
}

pub async fn reconcile(
    engine: &SqliteRawEngine,
    app: &AppHandle,
    source_id: i64,
    lib: &ItlFile,
) -> Result<PlaylistReconcileStats, PlaylistsError> {
    let mut stats = PlaylistReconcileStats::default();
    let total = lib.playlists().len() as u64;

    // Build a lookup from ITL track id (u32) → local track id (i64).
    // We look up by (sync_source_id, persistent_id) — but the in-memory
    // ITL track persistent_id is the u64 returned by Track::persistent_id().
    // So the map here is ITL track.id (u32, the internal id) → persistent_id (u64).
    let mut itl_to_pid: HashMap<u32, u64> = HashMap::with_capacity(lib.tracks().len());
    for t in lib.tracks() {
        itl_to_pid.insert(t.id(), t.persistent_id());
    }

    // Now build pid_hex → local i64 lookup from the DB.
    let pid_to_local = load_track_id_map(engine, source_id).await?;

    let mut keep: Vec<String> = Vec::with_capacity(lib.playlists().len());
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
        keep.push(format!("{:016x}", pid));

        let (kind, smart_rule_json) = classify(p);

        // Translate ITL u32 track ids to local i64 ids, skipping ones
        // we didn't import (because their persistent_id was zero or
        // their path didn't remap).
        let track_entries: Vec<i64> = p
            .track_ids()
            .iter()
            .filter_map(|itl_id| {
                let pid = itl_to_pid.get(itl_id)?;
                let hex = format!("{:016x}", pid);
                pid_to_local.get(&hex).copied()
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

        let existed = playlists::by_persistent_id(engine, source_id, &format!("{:016x}", pid))
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

    // Second pass: link parents. Load pid → local id for playlists now
    // that everything is inserted.
    let pid_to_local_pl = load_playlist_id_map(engine, source_id).await?;
    for (child_id, parent_pid) in pending_parent_links {
        let parent_local = pid_to_local_pl
            .get(&format!("{:016x}", parent_pid))
            .copied();
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

async fn load_track_id_map(
    engine: &SqliteRawEngine,
    source_id: i64,
) -> Result<HashMap<String, i64>, PlaylistsError> {
    let sql = "SELECT id, persistent_id FROM tracks WHERE sync_source_id = ?";
    let rows = engine
        .raw_sql_query(sql, &[FilterValue::Int(source_id)])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let v = r.into_json();
        let id = v.get("id").and_then(|v| v.as_i64());
        let pid = v
            .get("persistent_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let (Some(id), Some(pid)) = (id, pid) {
            out.insert(pid, id);
        }
    }
    Ok(out)
}

async fn load_playlist_id_map(
    engine: &SqliteRawEngine,
    source_id: i64,
) -> Result<HashMap<String, i64>, PlaylistsError> {
    let sql = "SELECT id, persistent_id FROM playlists WHERE sync_source_id = ?";
    let rows = engine
        .raw_sql_query(sql, &[FilterValue::Int(source_id)])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let v = r.into_json();
        let id = v.get("id").and_then(|v| v.as_i64());
        let pid = v
            .get("persistent_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let (Some(id), Some(pid)) = (id, pid) {
            out.insert(pid, id);
        }
    }
    Ok(out)
}

impl From<TracksError> for PlaylistsError {
    fn from(e: TracksError) -> Self {
        PlaylistsError::Query(anyhow::Error::msg(e.to_string()))
    }
}
