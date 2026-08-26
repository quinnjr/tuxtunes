//! Track-side reconciler. Reads every track from an `itl_rs::ItlFile`,
//! remaps its path, applies conflict rules, and writes to SQLite.

use crate::db::tracks::{self, ItlTrackUpsert, LocalTrackForSync, TracksError};
use crate::sync::conflict::{self, ConflictRules, Decision};
use crate::sync::events::{SyncPhase, SyncProgress, SyncWarning, WarningKind};
use crate::sync::import_log::{LogLevel, LogSink};
use crate::sync::observer::SyncObserver;
use crate::sync::path_map::{self, PathMapError, PathMapping};
use itl_rs::ItlFile;
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub struct TrackReconcileStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub warnings: u64,
    /// Entries whose ` N` collision-suffix file was gone but whose base
    /// file existed: relinked to it (unclaimed) or merged into the entry
    /// that already owns it (see `TrackReconcileOutcome::aliases`).
    pub relinked: u64,
    pub merged: u64,
}

/// Everything the playlist pass needs from the track pass.
#[derive(Debug, Default)]
pub struct TrackReconcileOutcome {
    pub stats: TrackReconcileStats,
    pub ingest_candidates: Vec<IngestCandidate>,
    /// `dropped_pid → surviving_pid` for entries merged into another
    /// track because both resolved to the same file. Playlists that
    /// referenced the dropped pid should point at the survivor.
    pub aliases: HashMap<u64, u64>,
}

/// `(track_id, source_path)` for a track that should be handed off to
/// the ingest worker when `SyncSource.auto_copy_files` is true.
#[derive(Debug, Clone)]
pub struct IngestCandidate {
    pub track_id: i64,
    pub source_path: PathBuf,
}

/// Reconcile every track in `lib` into `engine`. Emits progress + warning
/// events via `obs` and writes human-readable lines to `log`. Returns
/// aggregate counts and ingest candidates.
pub async fn reconcile(
    engine: &SqliteRawEngine,
    obs: &dyn SyncObserver,
    source_id: i64,
    lib: &ItlFile,
    mappings: &[PathMapping],
    rules: &ConflictRules,
    log: LogSink<'_>,
) -> Result<TrackReconcileOutcome, TracksError> {
    let mut stats = TrackReconcileStats::default();
    let mut ingest_candidates = Vec::new();
    let mut aliases: HashMap<u64, u64> = HashMap::new();
    let total = lib.tracks().len() as u64;
    // Cap how many individual warnings we surface — both as `sync:warning`
    // events and as log lines — so a library with no path mappings (or one
    // riddled with duplicates) can't flood the UI's IPC channel or the log
    // file. The remainder is reported once as a summary after the walk.
    const WARN_CAP: u64 = 50;
    let mut unmappable_throttle = WarnThrottle::new(WARN_CAP);
    let mut duplicate_throttle = WarnThrottle::new(WARN_CAP);

    // One SELECT up-front replaces N per-track by_persistent_id SELECTs.
    let local_map = tracks::load_local_state_map(engine, source_id).await?;
    let mut keep: Vec<u64> = Vec::with_capacity(lib.tracks().len());

    // persistent_ids and resolved file paths handled this run. `local_map`
    // is a pre-run snapshot, so a .itl that lists the same persistent_id (or
    // two entries resolving to the same file) would route both copies to the
    // insert branch and the second would violate the tracks.persistent_id /
    // tracks.file_path UNIQUE constraints, aborting the whole import. Real
    // iTunes libraries contain such duplicates, so skip any repeat.
    let mut seen: HashSet<u64> = HashSet::with_capacity(lib.tracks().len());
    // Resolved path → pid that claimed it, so a later entry resolving to
    // the same file can be aliased to the claimant instead of dropped.
    let mut seen_paths: HashMap<String, u64> = HashMap::with_capacity(lib.tracks().len());

    // Entries whose mapped file is gone but whose collision-suffix base
    // file exists are held back and applied after every direct entry,
    // so the entry that genuinely owns the base file always claims it
    // first and the suffixed one becomes an alias rather than a UNIQUE
    // violation on `tracks.file_path`.
    let mut deferred: Vec<(usize, String, String)> = Vec::new();

    for (idx, t) in lib.tracks().iter().enumerate() {
        if idx % 250 == 0 {
            obs.progress(&SyncProgress {
                source_id,
                phase: SyncPhase::ApplyingTracks,
                current: idx as u64,
                total,
                message: format!("{idx} / {total}"),
            });
            log(LogLevel::Info, &format!("applying tracks {idx} / {total}"));
        }

        let pid = t.persistent_id();
        if pid == 0 {
            stats.warnings += 1;
            continue;
        }

        if !seen.insert(pid) {
            if duplicate_throttle.allow() {
                let detail = format!(
                    "duplicate persistent_id {pid:016x} ({:?}); skipping repeat",
                    t.title()
                );
                obs.warning(&SyncWarning {
                    source_id,
                    kind: WarningKind::DuplicateTrack,
                    detail: detail.clone(),
                });
                log(LogLevel::Warn, &format!("duplicate track: {detail}"));
            }
            stats.warnings += 1;
            continue;
        }

        let raw_path = t.local_path().unwrap_or("");
        let mapped = match path_map::remap(raw_path, mappings) {
            Ok(p) => p,
            Err(PathMapError::Unmappable(reason)) => {
                if unmappable_throttle.allow() {
                    let detail = format!("track {pid:016x} ({:?}): {reason}", t.title());
                    obs.warning(&SyncWarning {
                        source_id,
                        kind: WarningKind::UnmappablePath,
                        detail: detail.clone(),
                    });
                    log(LogLevel::Warn, &format!("unmappable path: {detail}"));
                }
                stats.warnings += 1;
                continue;
            }
        };

        if let Some(base) = crate::fs::relink::dedupe_suffix_candidate(Path::new(&mapped)) {
            deferred.push((
                idx,
                raw_path.to_string(),
                base.to_string_lossy().into_owned(),
            ));
            continue;
        }

        apply_entry(
            engine,
            obs,
            source_id,
            t,
            raw_path,
            mapped,
            rules,
            &local_map,
            &mut seen_paths,
            &mut keep,
            &mut aliases,
            &mut stats,
            &mut ingest_candidates,
            &mut duplicate_throttle,
            log,
        )
        .await?;
    }

    if !deferred.is_empty() {
        log(
            LogLevel::Info,
            &format!(
                "relinking {} collision-suffix entries to surviving base files",
                deferred.len()
            ),
        );
    }
    for (idx, raw_path, base) in deferred {
        let t = &lib.tracks()[idx];
        let pid = t.persistent_id();
        if let Some(&owner) = seen_paths.get(&base) {
            // The base file's own entry is in the library: fold this
            // one into it so playlists keep the song.
            aliases.insert(pid, owner);
            stats.merged += 1;
            continue;
        }
        stats.relinked += 1;
        apply_entry(
            engine,
            obs,
            source_id,
            t,
            &raw_path,
            base,
            rules,
            &local_map,
            &mut seen_paths,
            &mut keep,
            &mut aliases,
            &mut stats,
            &mut ingest_candidates,
            &mut duplicate_throttle,
            log,
        )
        .await?;
    }
    if stats.relinked + stats.merged > 0 {
        log(
            LogLevel::Info,
            &format!(
                "collision-suffix recovery: {} relinked, {} merged into existing entries",
                stats.relinked, stats.merged
            ),
        );
    }

    // One summary per throttled category, so the IPC channel and the log see a
    // bounded number of warning events regardless of library size.
    if unmappable_throttle.suppressed() > 0 {
        let n = unmappable_throttle.suppressed();
        let detail = format!(
            "{n} further unmappable {} suppressed",
            if n == 1 { "path" } else { "paths" }
        );
        obs.warning(&SyncWarning {
            source_id,
            kind: WarningKind::UnmappablePath,
            detail: detail.clone(),
        });
        log(LogLevel::Warn, &detail);
    }
    if duplicate_throttle.suppressed() > 0 {
        let n = duplicate_throttle.suppressed();
        let detail = format!(
            "{n} further duplicate {} skipped",
            if n == 1 { "track" } else { "tracks" }
        );
        obs.warning(&SyncWarning {
            source_id,
            kind: WarningKind::DuplicateTrack,
            detail: detail.clone(),
        });
        log(LogLevel::Warn, &detail);
    }

    if rules.deletes == crate::sync::conflict::DeleteStrategy::Respect {
        let deleted = tracks::delete_missing(engine, source_id, &keep).await?;
        stats.deleted = deleted;
    }

    Ok(TrackReconcileOutcome {
        stats,
        ingest_candidates,
        aliases,
    })
}

/// Insert or update one library entry whose file path has been
/// resolved to `mapped`. A second entry resolving to a path already
/// claimed this run is aliased to the claimant and skipped.
#[allow(clippy::too_many_arguments)]
async fn apply_entry(
    engine: &SqliteRawEngine,
    obs: &dyn SyncObserver,
    source_id: i64,
    t: &itl_rs::Track,
    raw_path: &str,
    mapped: String,
    rules: &ConflictRules,
    local_map: &std::collections::HashMap<u64, LocalTrackForSync>,
    seen_paths: &mut HashMap<String, u64>,
    keep: &mut Vec<u64>,
    aliases: &mut HashMap<u64, u64>,
    stats: &mut TrackReconcileStats,
    ingest_candidates: &mut Vec<IngestCandidate>,
    duplicate_throttle: &mut WarnThrottle,
    log: LogSink<'_>,
) -> Result<(), TracksError> {
    let pid = t.persistent_id();
    if let Some(&owner) = seen_paths.get(&mapped) {
        aliases.insert(pid, owner);
        if duplicate_throttle.allow() {
            let detail = format!(
                "duplicate file path {mapped:?} (pid {pid:016x}); merged into {owner:016x}"
            );
            obs.warning(&SyncWarning {
                source_id,
                kind: WarningKind::DuplicateTrack,
                detail: detail.clone(),
            });
            log(LogLevel::Warn, &format!("duplicate track: {detail}"));
        }
        stats.warnings += 1;
        return Ok(());
    }
    seen_paths.insert(mapped.clone(), pid);
    keep.push(pid);

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

    match local_map.get(&pid) {
        None => {
            let track_id = tracks::insert_from_itl(engine, &upsert).await?;
            stats.inserted += 1;
            ingest_candidates.push(IngestCandidate {
                track_id,
                source_path: PathBuf::from(raw_path),
            });
        }
        Some(local) => {
            let (resolved_rating, resolved_play_count) = resolve_user_state(&upsert, local, rules);
            tracks::update_descriptive_fields(
                engine,
                local.id,
                &upsert,
                resolved_rating,
                resolved_play_count,
            )
            .await?;
            stats.updated += 1;
            ingest_candidates.push(IngestCandidate {
                track_id: local.id,
                source_path: PathBuf::from(raw_path),
            });
        }
    }
    Ok(())
}

/// Bounds how many individual warnings of one category are surfaced per run.
/// The first `cap` are allowed (emitted as `sync:warning` events and written
/// to the log); the rest are counted so the caller can report a single
/// summary. Keeps a pathological library from flooding the UI's IPC channel
/// and the log file.
struct WarnThrottle {
    shown: u64,
    suppressed: u64,
    cap: u64,
}

impl WarnThrottle {
    fn new(cap: u64) -> Self {
        Self {
            shown: 0,
            suppressed: 0,
            cap,
        }
    }

    /// Record a warning; returns true if it should be surfaced individually.
    fn allow(&mut self) -> bool {
        if self.shown < self.cap {
            self.shown += 1;
            true
        } else {
            self.suppressed += 1;
            false
        }
    }

    fn suppressed(&self) -> u64 {
        self.suppressed
    }
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
    // Integration-level tests live in tests/sync_smoke.rs, since the full
    // reconcile path needs a real ItlFile fixture. We only unit-test the pure
    // helper here.

    #[test]
    fn warn_throttle_allows_up_to_cap_then_counts_suppressed() {
        let mut t = WarnThrottle::new(2);
        assert!(t.allow()); // 1st
        assert!(t.allow()); // 2nd
        assert!(!t.allow()); // 3rd -> suppressed
        assert!(!t.allow()); // 4th -> suppressed
        assert_eq!(t.suppressed(), 2);
    }

    #[test]
    fn warn_throttle_exactly_fills_cap_without_suppressing() {
        let mut t = WarnThrottle::new(3);
        assert!(t.allow());
        assert!(t.allow());
        assert!(t.allow()); // 3rd fills the cap exactly
        assert_eq!(t.suppressed(), 0);
        assert!(!t.allow()); // 4th overflows
        assert_eq!(t.suppressed(), 1);
    }

    #[test]
    fn warn_throttle_reports_no_suppression_under_cap() {
        let mut t = WarnThrottle::new(5);
        assert!(t.allow());
        assert!(t.allow());
        assert_eq!(t.suppressed(), 0);
    }

    #[test]
    fn warn_throttle_zero_cap_suppresses_everything() {
        let mut t = WarnThrottle::new(0);
        assert!(!t.allow());
        assert!(!t.allow());
        assert_eq!(t.suppressed(), 2);
    }

    #[test]
    fn nz_filters_zeros() {
        assert_eq!(nz(0), None);
        assert_eq!(nz(-1), None);
        assert_eq!(nz(7), Some(7));
    }
}
