//! Play-count / skip-count decision rule.
//!
//! When a track ends or is skipped, decide which counter to increment.
//! iTunes-compatible rule: counts as a play if the user heard >= 30s OR
//! >= 50% of the duration (whichever is later). Otherwise it's a skip.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountDecision {
    Play,
    Skip,
    None,
}

/// Given the position at stop time and the total track duration, decide
/// whether to bump the play count, the skip count, or neither.
pub fn decide(position_ms: i64, duration_ms: i64) -> CountDecision {
    if position_ms <= 0 {
        return CountDecision::None;
    }
    let thirty_seconds_ms = 30_000;
    let half_duration_ms = duration_ms / 2;
    let threshold = thirty_seconds_ms.max(half_duration_ms);
    if position_ms >= threshold {
        CountDecision::Play
    } else {
        CountDecision::Skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_position_is_none() {
        assert_eq!(decide(0, 120_000), CountDecision::None);
    }

    #[test]
    fn negative_position_is_none() {
        // Defensive — shouldn't happen but the branch needs to be exercised
        // so the None variant is non-dead in non-test builds.
        assert_eq!(decide(-1, 120_000), CountDecision::None);
    }

    #[test]
    fn early_stop_on_long_track_is_skip() {
        // 10s into a 4-minute track — below both 30s and 50% of 240s (= 120s).
        assert_eq!(decide(10_000, 240_000), CountDecision::Skip);
    }

    #[test]
    fn past_30s_on_long_track_is_still_skip_if_under_half() {
        // 45s into a 4-minute track — past 30s but only 19% of duration.
        // Rule: threshold is max(30s, half), so threshold = 120s, 45s < 120s.
        assert_eq!(decide(45_000, 240_000), CountDecision::Skip);
    }

    #[test]
    fn past_half_on_long_track_is_play() {
        // 121s into a 4-minute track — past half.
        assert_eq!(decide(121_000, 240_000), CountDecision::Play);
    }

    #[test]
    fn past_30s_on_short_track_is_play() {
        // 31s into a 50s track — past 30s, and 30s > half(25s). Play.
        assert_eq!(decide(31_000, 50_000), CountDecision::Play);
    }

    #[test]
    fn under_30s_on_short_track_is_skip() {
        // 20s into a 50s track. 20s < 30s (= max(30s, 25s)).
        assert_eq!(decide(20_000, 50_000), CountDecision::Skip);
    }
}
