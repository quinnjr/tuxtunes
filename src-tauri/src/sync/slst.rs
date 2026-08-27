//! Decoder for iTunes' binary smart-playlist criteria (`SLst`) and the
//! companion `Smart Info` flag block, as exposed by
//! `itl_rs::Playlist::smart_criteria()` / `smart_info()`.
//!
//! The layout is the one libgpod documents for iPod `iTunesDB` mhod
//! types 50/51 and it is unchanged in iTunes 12 library files (verified
//! against 419 playlists: every blob parses to its exact length):
//!
//! ```text
//! SLst header (136 bytes)
//!   0   "SLst"
//!   4   u32 BE version (0x0001_0001)
//!   8   u32 BE rule count
//!   12  u32 BE conjunction: 0 = match ALL, 1 = match ANY
//!   16  120 zero bytes
//! rule (56 bytes + value)
//!   0   u32 BE field
//!   4   u32 BE action
//!   8   44 bytes (usually zero)
//!   52  u32 BE value length
//!   56  value: UTF-16BE string (string actions), a 68-byte numeric
//!       block (six u64 BE: from, from_date, from_units, to, to_date,
//!       to_units, then 20 zero bytes), or a nested SLst (field 0).
//! Smart Info
//!   0 live updating, 1 rules enabled, 2 limit enabled, 3 limit unit
//!   (1 min, 2 MB, 3 songs, 4 h, 5 GB), 4 u32 BE limit sort, 8 u32 BE
//!   limit value, 12 match checked only, 13 reverse sort.
//! ```

use crate::db::smart::{
    Condition, ConditionGroup, LeafCondition, Limit, LimitUnit, Op, SelectionMode, SmartRule,
    TimeUnit, Value,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SlstError {
    #[error("not an SLst blob")]
    BadMagic,
    #[error("truncated SLst blob at offset {0}")]
    Truncated(usize),
    #[error("rule uses field {field:#x} which TuxTunes cannot evaluate")]
    UnsupportedField { field: u32 },
    #[error("rule uses action {action:#x} on field {field:#x} which TuxTunes cannot evaluate")]
    UnsupportedAction { field: u32, action: u32 },
    #[error("smart playlist has no evaluable rules")]
    Empty,
}

/// Everything the reconciler wants to know about a decoded playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub rule: SmartRule,
    /// Rules dropped because TuxTunes has no equivalent (media kind,
    /// playlist membership, cloud status, …). Non-empty means the
    /// imported rule may match a superset of what iTunes showed.
    pub dropped: Vec<String>,
}

const HEADER_LEN: usize = 136;
const RULE_HEAD_LEN: usize = 56;

// libgpod ITDB_SPLFIELD_* / ITDB_SPLACTION_* codes.
const FIELD_SUBEXPRESSION: u32 = 0x00;
const FIELD_MEDIA_KIND: u32 = 0x3c;
const ACTION_STRING_FLAG: u32 = 0x0100_0000;
const ACTION_NEGATE_FLAG: u32 = 0x0200_0000;
const ACTION_BASE_MASK: u32 = 0x00ff_ffff;
const ACTION_IS: u32 = 0x01;
const ACTION_CONTAINS: u32 = 0x02;
const ACTION_STARTS_WITH: u32 = 0x04;
const ACTION_ENDS_WITH: u32 = 0x08;
const ACTION_GREATER: u32 = 0x10;
const ACTION_LESS: u32 = 0x40;
const ACTION_IN_RANGE: u32 = 0x100;
const ACTION_IN_THE_LAST: u32 = 0x200;

/// Seconds between the Mac epoch (1904-01-01) and the Unix epoch.
const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;

fn u32_at(b: &[u8], off: usize) -> Result<u32, SlstError> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(SlstError::Truncated(off))
}

fn u64_at(b: &[u8], off: usize) -> Result<u64, SlstError> {
    b.get(off..off + 8)
        .map(|s| u64::from_be_bytes(s.try_into().expect("8 bytes")))
        .ok_or(SlstError::Truncated(off))
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Text,
    Int,
    Date,
    Bool,
}

/// iTunes field code → (TuxTunes field, kind). Anything else is dropped.
fn field_meta(code: u32) -> Option<(&'static str, Kind)> {
    Some(match code {
        0x02 => ("title", Kind::Text),
        0x03 => ("album", Kind::Text),
        0x04 => ("artist", Kind::Text),
        0x05 => ("bit_rate", Kind::Int),
        0x06 => ("sample_rate", Kind::Int),
        0x07 => ("year", Kind::Int),
        0x08 => ("genre", Kind::Text),
        0x09 => ("kind", Kind::Text),
        0x0b => ("track_number", Kind::Int),
        0x0c => ("size_bytes", Kind::Int),
        0x0d => ("duration_ms", Kind::Int),
        0x0e => ("comment", Kind::Text),
        0x10 => ("date_added", Kind::Date),
        0x12 => ("composer", Kind::Text),
        0x16 => ("play_count", Kind::Int),
        0x17 => ("last_played", Kind::Date),
        0x18 => ("disc_number", Kind::Int),
        0x19 => ("rating", Kind::Int),
        0x23 => ("bpm", Kind::Int),
        0x44 => ("skip_count", Kind::Int),
        0x45 => ("last_skipped", Kind::Date),
        0x47 => ("album_artist", Kind::Text),
        0x9a => ("loved", Kind::Bool),
        _ => return None,
    })
}

fn field_name(code: u32) -> String {
    match code {
        0x28 => "playlist".into(),
        0x3c => "media_kind".into(),
        0x85 => "cloud_status".into(),
        0x1f => "compilation".into(),
        0x27 => "grouping".into(),
        other => format!("field_{other:#x}"),
    }
}

struct Numeric {
    from: u64,
    from_date: u64,
    from_units: u64,
    to: u64,
}

fn numeric(value: &[u8]) -> Result<Numeric, SlstError> {
    Ok(Numeric {
        from: u64_at(value, 0)?,
        from_date: u64_at(value, 8)?,
        from_units: u64_at(value, 16)?,
        to: u64_at(value, 24)?,
    })
}

fn relative(n: &Numeric) -> Value {
    // iTunes stores the count negated in `from_date` and the unit as
    // seconds-per-unit in `from_units`.
    let count = (n.from_date as i64).unsigned_abs().max(1) as i64;
    let unit = match n.from_units {
        604_800 => TimeUnit::Weeks,
        2_628_000 => TimeUnit::Months,
        _ => TimeUnit::Days,
    };
    Value::Relative { n: count, unit }
}

fn mac_to_unix(secs: u64) -> i64 {
    (secs as i64) - MAC_EPOCH_OFFSET
}

fn leaf(field: u32, action: u32, value: &[u8]) -> Result<Option<LeafCondition>, SlstError> {
    let Some((name, kind)) = field_meta(field) else {
        return Ok(None);
    };
    let negate = action & ACTION_NEGATE_FLAG != 0;
    let is_string = action & ACTION_STRING_FLAG != 0;
    let base = action & ACTION_BASE_MASK;
    let unsupported = || SlstError::UnsupportedAction { field, action };

    let (op, value) = match (kind, is_string, base) {
        (Kind::Text, true, ACTION_IS) => (
            if negate { Op::IsNot } else { Op::Is },
            Value::Text(utf16(value)),
        ),
        (Kind::Text, true, ACTION_CONTAINS) => (
            if negate {
                Op::NotContains
            } else {
                Op::Contains
            },
            Value::Text(utf16(value)),
        ),
        (Kind::Text, true, ACTION_STARTS_WITH) if !negate => {
            (Op::StartsWith, Value::Text(utf16(value)))
        }
        (Kind::Text, true, ACTION_ENDS_WITH) if !negate => {
            (Op::EndsWith, Value::Text(utf16(value)))
        }
        (Kind::Int, false, ACTION_IS) => {
            let n = numeric(value)?;
            (
                if negate { Op::IsNot } else { Op::Is },
                Value::Int(n.from as i64),
            )
        }
        (Kind::Int, false, ACTION_GREATER) if !negate => {
            (Op::Greater, Value::Int(numeric(value)?.from as i64))
        }
        (Kind::Int, false, ACTION_LESS) if !negate => {
            (Op::Less, Value::Int(numeric(value)?.from as i64))
        }
        (Kind::Int, false, ACTION_IN_RANGE) if !negate => {
            let n = numeric(value)?;
            (
                Op::InRange,
                Value::Range {
                    from: n.from as i64,
                    to: n.to as i64,
                },
            )
        }
        (Kind::Date, false, ACTION_IN_THE_LAST) => (
            if negate {
                Op::NotInTheLast
            } else {
                Op::InTheLast
            },
            relative(&numeric(value)?),
        ),
        (Kind::Date, false, ACTION_GREATER) if !negate => {
            (Op::Greater, Value::Int(mac_to_unix(numeric(value)?.from)))
        }
        (Kind::Date, false, ACTION_LESS) if !negate => {
            (Op::Less, Value::Int(mac_to_unix(numeric(value)?.from)))
        }
        (Kind::Date, false, ACTION_IN_RANGE) if !negate => {
            let n = numeric(value)?;
            (
                Op::InRange,
                Value::Range {
                    from: mac_to_unix(n.from),
                    to: mac_to_unix(n.to),
                },
            )
        }
        (Kind::Bool, false, ACTION_IS) => {
            let n = numeric(value)?;
            (
                if negate { Op::IsNot } else { Op::Is },
                Value::Bool(n.from != 0),
            )
        }
        _ => return Err(unsupported()),
    };
    Ok(Some(LeafCondition {
        field: name.to_string(),
        op,
        value,
    }))
}

fn utf16(value: &[u8]) -> String {
    let units: Vec<u16> = value
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn group(b: &[u8], dropped: &mut Vec<String>) -> Result<ConditionGroup, SlstError> {
    if b.len() < 4 || &b[..4] != b"SLst" {
        return Err(SlstError::BadMagic);
    }
    let count = u32_at(b, 8)? as usize;
    let match_all = u32_at(b, 12)? == 0;
    let mut children = Vec::with_capacity(count);
    let mut off = HEADER_LEN;
    for _ in 0..count {
        let field = u32_at(b, off)?;
        let action = u32_at(b, off + 4)?;
        let len = u32_at(b, off + 52)? as usize;
        let value = b
            .get(off + RULE_HEAD_LEN..off + RULE_HEAD_LEN + len)
            .ok_or(SlstError::Truncated(off + RULE_HEAD_LEN))?;
        off += RULE_HEAD_LEN + len;

        if field == FIELD_SUBEXPRESSION {
            let sub = group(value, dropped)?;
            if !sub.children.is_empty() {
                children.push(Condition::Group(sub));
            }
            continue;
        }
        match leaf(field, action, value) {
            Ok(Some(l)) => children.push(Condition::Leaf(l)),
            // iTunes adds an implicit "media kind is music" rule to every
            // playlist; TuxTunes is music-only, so dropping it changes
            // nothing and isn't worth a warning.
            Ok(None) if field == FIELD_MEDIA_KIND => {}
            Ok(None) => dropped.push(field_name(field)),
            Err(SlstError::UnsupportedAction { .. }) => dropped.push(format!(
                "{} (action {action:#x})",
                field_meta(field).map(|m| m.0).unwrap_or("?")
            )),
            Err(e) => return Err(e),
        }
    }
    Ok(ConditionGroup {
        match_all,
        children,
    })
}

fn limit(info: &[u8]) -> Option<Limit> {
    if info.len() < 12 || info[2] == 0 {
        return None;
    }
    let unit = match info[3] {
        1 => LimitUnit::Minutes,
        2 => LimitUnit::Mb,
        4 => LimitUnit::Hours,
        5 => LimitUnit::Gb,
        _ => LimitUnit::Songs,
    };
    let sort = u32::from_be_bytes([info[4], info[5], info[6], info[7]]);
    let value = u32::from_be_bytes([info[8], info[9], info[10], info[11]]);
    let selected_by = Some(match sort {
        0x03 => SelectionMode::SongName,
        0x04 => SelectionMode::Album,
        0x05 => SelectionMode::Artist,
        0x07 => SelectionMode::Genre,
        0x10 => SelectionMode::MostRecentlyAdded,
        0x14 | 0x19 => SelectionMode::MostOftenPlayed,
        0x15 => SelectionMode::MostRecentlyPlayed,
        0x17 => SelectionMode::HighestRating,
        _ => SelectionMode::Random,
    });
    Some(Limit {
        value: value.max(1),
        unit,
        selected_by,
    })
}

/// Decode a playlist's criteria (+ optional info block) into a
/// TuxTunes rule. Rules TuxTunes cannot evaluate are dropped and
/// reported in `dropped`; a playlist left with no evaluable rule at
/// all is an error so the caller can fall back to a static list.
pub fn decode(criteria: &[u8], info: Option<&[u8]>) -> Result<Decoded, SlstError> {
    let mut dropped = Vec::new();
    let root = group(criteria, &mut dropped)?;
    if root.children.is_empty() {
        return Err(SlstError::Empty);
    }
    let info = info.unwrap_or(&[]);
    let live_updating = info.first().is_none_or(|b| *b != 0);
    Ok(Decoded {
        rule: SmartRule {
            match_all: root.match_all,
            live_updating,
            limit: limit(info),
            root,
        },
        dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- tiny encoder so the tests don't hard-code hex -----
    fn header(count: u32, any: bool) -> Vec<u8> {
        let mut b = b"SLst".to_vec();
        b.extend_from_slice(&0x0001_0001u32.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
        b.extend_from_slice(&(any as u32).to_be_bytes());
        b.resize(HEADER_LEN, 0);
        b
    }
    fn rule(field: u32, action: u32, value: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&field.to_be_bytes());
        b.extend_from_slice(&action.to_be_bytes());
        b.resize(52, 0);
        b.extend_from_slice(&(value.len() as u32).to_be_bytes());
        b.extend_from_slice(value);
        b
    }
    fn num(from: u64, from_date: u64, from_units: u64, to: u64) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [from, from_date, from_units, to, 0, 1] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.resize(68, 0);
        b
    }
    fn s(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }
    fn info(live: u8, limit_on: u8, unit: u8, sort: u32, value: u32) -> Vec<u8> {
        let mut b = vec![live, 1, limit_on, unit];
        b.extend_from_slice(&sort.to_be_bytes());
        b.extend_from_slice(&value.to_be_bytes());
        b.resize(98, 0);
        b
    }

    #[test]
    fn decodes_itunes_artist_playlist_shape() {
        // [media kind is 1 OR media kind is 32] AND [artist contains "10 Years"]
        let mut inner1 = header(2, true);
        inner1.extend(rule(0x3c, 0x01, &num(1, 0, 1, 1)));
        inner1.extend(rule(0x3c, 0x01, &num(32, 0, 1, 32)));
        let mut inner2 = header(1, false);
        inner2.extend(rule(0x04, 0x0100_0002, &s("10 Years")));
        let mut blob = header(2, false);
        blob.extend(rule(0, 0x01, &inner1));
        blob.extend(rule(0, 0x01, &inner2));

        let d = decode(&blob, Some(&info(1, 0, 3, 2, 25))).unwrap();
        assert!(d.rule.match_all);
        assert!(d.rule.live_updating);
        assert_eq!(d.rule.limit, None);
        assert!(
            d.dropped.is_empty(),
            "media kind is implicit, not a warning"
        );
        // The media-kind-only group vanished entirely; the artist group stays.
        assert_eq!(d.rule.root.children.len(), 1);
        match &d.rule.root.children[0] {
            Condition::Group(g) => {
                assert!(g.match_all);
                assert_eq!(
                    g.children,
                    vec![Condition::Leaf(LeafCondition {
                        field: "artist".into(),
                        op: Op::Contains,
                        value: Value::Text("10 Years".into()),
                    })]
                );
            }
            other => panic!("expected group, got {other:?}"),
        }
    }

    #[test]
    fn decodes_numeric_date_and_limit_forms() {
        let mut blob = header(6, true);
        blob.extend(rule(0x19, 0x01, &num(80, 0, 1, 80))); // rating is 80
        blob.extend(rule(0x16, 0x0200_0001, &num(0, 0, 1, 0))); // play count is not 0
        blob.extend(rule(0x07, 0x100, &num(1990, 0, 1, 1999))); // year in range
        blob.extend(rule(0x17, 0x200, &num(0x2dae, (-2i64) as u64, 604_800, 0))); // last played in the last 2 weeks
        blob.extend(rule(0x10, 0x10, &num(3_000_000_000, 0, 1, 0))); // date added > (mac secs)
        blob.extend(rule(0x03, 0x0300_0002, &s("Live"))); // album does not contain
        let d = decode(&blob, Some(&info(0, 1, 4, 0x17, 3))).unwrap();
        assert!(!d.rule.match_all);
        assert!(!d.rule.live_updating);
        assert_eq!(
            d.rule.limit,
            Some(Limit {
                value: 3,
                unit: LimitUnit::Hours,
                selected_by: Some(SelectionMode::HighestRating)
            })
        );
        assert!(d.dropped.is_empty());
        let leaves: Vec<(&str, Op, Value)> = d
            .rule
            .root
            .children
            .iter()
            .map(|c| match c {
                Condition::Leaf(l) => (l.field.as_str(), l.op, l.value.clone()),
                _ => panic!(),
            })
            .collect();
        assert_eq!(leaves[0], ("rating", Op::Is, Value::Int(80)));
        assert_eq!(leaves[1], ("play_count", Op::IsNot, Value::Int(0)));
        assert_eq!(
            leaves[2],
            (
                "year",
                Op::InRange,
                Value::Range {
                    from: 1990,
                    to: 1999
                }
            )
        );
        assert_eq!(
            leaves[3],
            (
                "last_played",
                Op::InTheLast,
                Value::Relative {
                    n: 2,
                    unit: TimeUnit::Weeks
                }
            )
        );
        assert_eq!(
            leaves[4],
            (
                "date_added",
                Op::Greater,
                Value::Int(3_000_000_000 - MAC_EPOCH_OFFSET)
            )
        );
        assert_eq!(
            leaves[5],
            ("album", Op::NotContains, Value::Text("Live".into()))
        );
    }

    #[test]
    fn folder_style_playlist_rules_are_dropped_to_empty() {
        let mut blob = header(2, true);
        blob.extend(rule(0x28, 0x01, &num(1, 0, 1, 1)));
        blob.extend(rule(0x28, 0x01, &num(2, 0, 1, 2)));
        assert_eq!(decode(&blob, None), Err(SlstError::Empty));
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert_eq!(decode(b"nope", None), Err(SlstError::BadMagic));
        let mut blob = header(1, false);
        blob.extend(rule(0x04, 0x0100_0002, &s("x")));
        blob.truncate(blob.len() - 1);
        assert!(matches!(decode(&blob, None), Err(SlstError::Truncated(_))));
    }
}
