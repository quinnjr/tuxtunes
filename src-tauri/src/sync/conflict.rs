//! Per-field conflict resolution.
//!
//! On update, descriptive fields (title/artist/album/...) always take the
//! ITL source value. Six user-state fields consult a per-field strategy.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    PreferSource,
    PreferLocal,
    LastWriteWins,
}

#[allow(clippy::derivable_impls)]
impl Default for Strategy {
    fn default() -> Self {
        Self::PreferSource
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ConflictRules {
    pub rating: Strategy,
    pub play_count: Strategy,
    pub skip_count: Strategy,
    pub last_played: Strategy,
    pub last_skipped: Strategy,
    pub loved: Strategy,
    /// What to do with tracks that exist locally but not in the source.
    pub deletes: DeleteStrategy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeleteStrategy {
    /// Source wins: delete local rows that aren't in the ITL.
    #[default]
    Respect,
    /// Ignore: leave local-only rows alone.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    TakeSource,
    KeepLocal,
}

/// Resolve an integer field (rating, play_count, skip_count).
pub fn resolve_int(strategy: Strategy, source: i64, local: i64, source_wins_ts: bool) -> Decision {
    match strategy {
        Strategy::PreferSource => Decision::TakeSource,
        Strategy::PreferLocal => Decision::KeepLocal,
        Strategy::LastWriteWins => {
            if source == local {
                // Tie: no-op; either choice is fine.
                Decision::TakeSource
            } else if source_wins_ts {
                Decision::TakeSource
            } else {
                Decision::KeepLocal
            }
        }
    }
}

/// Resolve an Option<DateTime> field.
/// Returns `Decision::TakeSource` when the source value is "newer" per the
/// rule.
pub fn resolve_datetime(strategy: Strategy, source: Option<i64>, local: Option<i64>) -> Decision {
    match (strategy, source, local) {
        (Strategy::PreferSource, _, _) => Decision::TakeSource,
        (Strategy::PreferLocal, _, _) => Decision::KeepLocal,
        (Strategy::LastWriteWins, Some(s), Some(l)) => {
            if s >= l {
                Decision::TakeSource
            } else {
                Decision::KeepLocal
            }
        }
        (Strategy::LastWriteWins, Some(_), None) => Decision::TakeSource,
        (Strategy::LastWriteWins, None, Some(_)) => Decision::KeepLocal,
        (Strategy::LastWriteWins, None, None) => Decision::TakeSource,
    }
}

/// Resolve a boolean (`loved`).
pub fn resolve_bool(strategy: Strategy, source: bool, local: bool) -> Decision {
    match strategy {
        Strategy::PreferSource => Decision::TakeSource,
        Strategy::PreferLocal => Decision::KeepLocal,
        Strategy::LastWriteWins => {
            if source == local {
                Decision::TakeSource
            } else {
                // For boolean last-write-wins, caller must pass the
                // relevant timestamps via `resolve_datetime` instead.
                // Fallback: prefer source to stay deterministic.
                Decision::TakeSource
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefer_source_always_takes_source() {
        assert_eq!(
            resolve_int(Strategy::PreferSource, 5, 2, false),
            Decision::TakeSource
        );
        assert_eq!(
            resolve_bool(Strategy::PreferSource, true, false),
            Decision::TakeSource
        );
    }

    #[test]
    fn prefer_local_always_keeps_local() {
        assert_eq!(
            resolve_int(Strategy::PreferLocal, 5, 2, true),
            Decision::KeepLocal
        );
    }

    #[test]
    fn last_write_wins_uses_timestamp_flag() {
        assert_eq!(
            resolve_int(Strategy::LastWriteWins, 5, 2, true),
            Decision::TakeSource
        );
        assert_eq!(
            resolve_int(Strategy::LastWriteWins, 5, 2, false),
            Decision::KeepLocal
        );
    }

    #[test]
    fn lww_datetime_source_newer_wins() {
        let d = resolve_datetime(Strategy::LastWriteWins, Some(100), Some(50));
        assert_eq!(d, Decision::TakeSource);
    }

    #[test]
    fn lww_datetime_local_newer_keeps() {
        let d = resolve_datetime(Strategy::LastWriteWins, Some(50), Some(100));
        assert_eq!(d, Decision::KeepLocal);
    }

    #[test]
    fn lww_datetime_source_only_wins() {
        let d = resolve_datetime(Strategy::LastWriteWins, Some(50), None);
        assert_eq!(d, Decision::TakeSource);
    }

    #[test]
    fn default_rules_prefer_source_everywhere() {
        let r = ConflictRules::default();
        assert_eq!(r.rating, Strategy::PreferSource);
        assert_eq!(r.deletes, DeleteStrategy::Respect);
    }

    #[test]
    fn rules_serde_roundtrip() {
        let r = ConflictRules {
            rating: Strategy::PreferLocal,
            play_count: Strategy::LastWriteWins,
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ConflictRules = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn lww_int_tie_takes_source() {
        // Equal values should resolve TakeSource (no-op tie-break).
        assert_eq!(
            resolve_int(Strategy::LastWriteWins, 5, 5, false),
            Decision::TakeSource
        );
    }

    #[test]
    fn resolve_datetime_prefer_local_short_circuits() {
        // PreferLocal must keep local regardless of which side has a value.
        assert_eq!(
            resolve_datetime(Strategy::PreferLocal, Some(100), Some(50)),
            Decision::KeepLocal
        );
    }

    #[test]
    fn lww_datetime_neither_side_present_takes_source() {
        // No timestamps either side → deterministic TakeSource fallback.
        assert_eq!(
            resolve_datetime(Strategy::LastWriteWins, None, None),
            Decision::TakeSource
        );
    }

    #[test]
    fn resolve_bool_lww_inequality_takes_source() {
        // LWW on bool with no timestamp context defaults to source so
        // the resolution stays deterministic.
        assert_eq!(
            resolve_bool(Strategy::LastWriteWins, true, false),
            Decision::TakeSource
        );
    }

    #[test]
    fn resolve_bool_lww_tie_takes_source() {
        assert_eq!(
            resolve_bool(Strategy::LastWriteWins, true, true),
            Decision::TakeSource
        );
    }

    #[test]
    fn resolve_bool_prefer_local_keeps_local() {
        assert_eq!(
            resolve_bool(Strategy::PreferLocal, true, false),
            Decision::KeepLocal
        );
    }
}
