//! Track-side reconciler. Reads every track from an `itl_rs::ItlFile`,
//! remaps its path, applies conflict rules, and writes to SQLite.

use crate::db::tracks::{self, ItlTrackUpsert, LocalTrackForSync, TracksError};
use crate::sync::conflict::{self, ConflictRules, Decision};
use crate::sync::events::{SyncPhase, SyncProgress, SyncWarning, WarningKind};
use crate::sync::path_map::{self, PathMapError, PathMapping};
use itl_rs::ItlFile;
use prax_sqlite::raw::SqliteRawEngine;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Default, Clone, Copy)]
pub struct TrackReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
}

/// Reconcile every track in `lib` into `engine`. Emits progress + warning
/// events via `app`. Returns aggregate counts.
pub async fn reconcile(
    engine: &SqliteRawEngine,
    app: &AppHandle,
    source_id: i64,
    lib: &ItlFile,
    mappings: &[PathMapping],
    rules: &ConflictRules,
) -> Result<TrackReconcileStats, TracksError> {
    let mut stats = TrackReconcileStats::default();
    let total = lib.tracks().len() as u64;

    let mut keep_ids: Vec<String> = Vec::with_capacity(lib.tracks().len());

    for (idx, t) in lib.tracks().iter().enumerate() {
        if idx % 250 == 0 {
            let _ = app.emit(
                crate::sync::events::PROGRESS,
                SyncProgress {
                    source_id,
                    phase: SyncPhase::ApplyingTracks,
                    current: idx as u64,
                    total,
                    message: format!("{idx} / {total}"),
                },
            );
        }

        let pid = t.persistent_id();
        if pid == 0 {
            stats.warnings += 1;
            continue;
        }
        let pid_hex = format!("{:016x}", pid);

        // Path remapping.
        let raw_path = t.local_path().unwrap_or("");
        let mapped = match path_map::remap(raw_path, mappings) {
            Ok(p) => p,
            Err(PathMapError::Unmappable(reason)) => {
                let _ = app.emit(
                    crate::sync::events::WARNING,
                    SyncWarning {
                        source_id,
                        kind: WarningKind::UnmappablePath,
                        detail: format!("track {:016x} ({:?}): {reason}", pid, t.title()),
                    },
                );
                stats.warnings += 1;
                continue;
            }
        };

        keep_ids.push(pid_hex.clone());

        // Check for missing source file.
        if !std::path::Path::new(&mapped).exists() {
            let _ = app.emit(
                crate::sync::events::WARNING,
                SyncWarning {
                    source_id,
                    kind: WarningKind::MissingSourceFile,
                    detail: format!("{} (track {:016x})", mapped, pid),
                },
            );
            stats.warnings += 1;
        }

        let upsert = ItlTrackUpsert {
            persistent_id: pid,
            sync_source_id: source_id,
            title: t.title().unwrap_or(""),
            artist: t.artist(),
            album: t.album(),
            album_artist: t.album_artist(),
            composer: t.composer(),
            genre: t.genre(),
            kind: t.kind(),
            duration_ms: i64::from(t.duration_ms()),
            size_bytes: i64::from(t.size_bytes()),
            bit_rate: nz(i64::from(t.bit_rate())),
            sample_rate: nz(i64::from(t.sample_rate())),
            track_number: t.track_number().map(i64::from),
            disc_number: t.disc_number().map(i64::from),
            year: t.year().map(i64::from),
            bpm: t.bpm().map(i64::from),
            rating: i64::from(t.rating()),
            play_count: i64::from(t.play_count()),
            date_added_unix: t.date_added_unix(),
            file_path: &mapped,
            original_path: Some(raw_path),
        };

        match tracks::by_persistent_id(engine, source_id, &pid_hex).await? {
            None => {
                tracks::insert_from_itl(engine, &upsert).await?;
                stats.inserted += 1;
            }
            Some(local) => {
                let (resolved_rating, resolved_play_count) =
                    resolve_user_state(&upsert, &local, rules);
                tracks::update_descriptive_fields(
                    engine,
                    local.id,
                    &upsert,
                    resolved_rating,
                    resolved_play_count,
                    &mapped,
                )
                .await?;
                stats.updated += 1;
            }
        }
    }

    // Apply deletes.
    if rules.deletes == crate::sync::conflict::DeleteStrategy::Respect {
        let deleted = tracks::delete_missing(engine, source_id, &keep_ids).await?;
        stats.deleted = deleted;
    }

    Ok(stats)
}

fn nz(v: i64) -> Option<i64> {
    if v > 0 {
        Some(v)
    } else {
        None
    }
}

fn resolve_user_state(
    source: &ItlTrackUpsert<'_>,
    local: &LocalTrackForSync,
    rules: &ConflictRules,
) -> (i64, i64) {
    // For last-write-wins, we don't have ITL-side timestamps for ratings
    // or play counts separately, so source_wins_ts is driven by which
    // side has the larger count (recent activity = newer).
    let rating_decision = conflict::resolve_int(
        rules.rating,
        source.rating,
        local.rating,
        source.rating > local.rating,
    );
    let play_decision = conflict::resolve_int(
        rules.play_count,
        source.play_count,
        local.play_count,
        source.play_count > local.play_count,
    );
    let rating = match rating_decision {
        Decision::TakeSource => source.rating,
        Decision::KeepLocal => local.rating,
    };
    let play_count = match play_decision {
        Decision::TakeSource => source.play_count,
        Decision::KeepLocal => local.play_count,
    };
    (rating, play_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Integration-level tests live in tests/sync_integration.rs, since
    // the full reconcile path needs a real ItlFile fixture. We only
    // unit-test the pure helper here.

    #[test]
    fn nz_filters_zeros() {
        assert_eq!(nz(0), None);
        assert_eq!(nz(-1), None);
        assert_eq!(nz(7), Some(7));
    }
}
