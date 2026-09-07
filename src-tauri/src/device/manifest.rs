//! Diffing what the device should hold against what TuxTunes has
//! already written there.
//!
//! The existing side comes from `device_objects`, never from listing
//! the device. That is deliberate: a path with no manifest row belongs
//! to someone else, and [`SyncPlan::orphans`] — the only input to the
//! prune phase — can therefore never name it.

use crate::db::device_objects::DeviceObjectRow;
use std::collections::HashMap;

/// Manifest kind for a pushed audio file.
pub const KIND_TRACK: &str = "track";
/// Manifest kind for a written `.m3u8` (and its playlist object).
pub const KIND_PLAYLIST: &str = "playlist";

/// One object the current selection says should be on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    pub track_id: i64,
    pub persistent_id: Option<String>,
    pub device_path: String,
    /// `Track.file_hash`. `None` for a track we could not hash, which
    /// forces a re-push rather than a silent skip.
    pub source_hash: Option<String>,
    /// `copy:<codec>` for a bit-exact push, or the target codec.
    pub encoded_codec: String,
    pub size_bytes: i64,
    /// Host path to read from.
    pub source_path: String,
    /// Whether a manifest row already claims this device path.
    ///
    /// Gates the overwrite in the upload phase: without a row, the file
    /// on the device belongs to someone else and must not be clobbered.
    pub replaces_existing: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub adds: Vec<Desired>,
    /// `(existing row id, what it should become)`.
    pub replaces: Vec<(i64, Desired)>,
    pub unchanged: usize,
    /// Manifest rows no longer wanted. Only these are ever deleted.
    pub orphans: Vec<DeviceObjectRow>,
    pub bytes_out: u64,
}

/// Diff `desired` against the track rows of `existing`.
///
/// Playlist rows are left alone: they are diffed separately, after the
/// upload phase settles which tracks actually made it onto the device.
pub fn diff(desired: &[Desired], existing: &[DeviceObjectRow]) -> SyncPlan {
    let tracks: Vec<&DeviceObjectRow> = existing.iter().filter(|r| r.kind == KIND_TRACK).collect();
    let by_path: HashMap<&str, &DeviceObjectRow> = tracks
        .iter()
        .map(|r| (r.device_path.as_str(), *r))
        .collect();

    let mut plan = SyncPlan::default();
    let mut claimed: HashMap<&str, ()> = HashMap::new();

    for want in desired {
        match by_path.get(want.device_path.as_str()) {
            None => {
                plan.bytes_out += want.size_bytes.max(0) as u64;
                plan.adds.push(want.clone());
            }
            Some(have) => {
                claimed.insert(have.device_path.as_str(), ());
                let same = have.encoded_codec == want.encoded_codec
                    && match (&have.source_hash, &want.source_hash) {
                        (Some(a), Some(b)) => a == b,
                        // Neither side has a hash. The ITL importer
                        // never computes one, so this is the *normal*
                        // case for a synced library — treating it as
                        // "changed" would re-push the whole library on
                        // every run and then trip the free-space check.
                        // Size is the best available proxy.
                        (None, None) => have.size_bytes == want.size_bytes,
                        // One side is hashed and the other is not (a
                        // verify pass filled it in since). Equality
                        // cannot be proven, so re-push once; the next
                        // run compares hash to hash and settles.
                        _ => false,
                    };
                if same {
                    plan.unchanged += 1;
                } else {
                    plan.bytes_out += want.size_bytes.max(0) as u64;
                    // A manifest row claims this path, so overwriting
                    // it is ours to do.
                    let mut want = want.clone();
                    want.replaces_existing = true;
                    plan.replaces.push((have.id, want));
                }
            }
        }
    }

    for row in tracks {
        if !claimed.contains_key(row.device_path.as_str()) {
            plan.orphans.push((*row).clone());
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired_at(path: &str, hash: &str) -> Desired {
        desired_codec(path, hash, "copy:flac")
    }

    fn desired_codec(path: &str, hash: &str, codec: &str) -> Desired {
        Desired {
            track_id: 1,
            persistent_id: None,
            device_path: path.into(),
            source_hash: Some(hash.into()),
            encoded_codec: codec.into(),
            size_bytes: 100,
            source_path: "/lib/a.flac".into(),
            replaces_existing: false,
        }
    }

    fn existing_at(id: i64, path: &str, hash: &str, codec: &str) -> DeviceObjectRow {
        DeviceObjectRow {
            id,
            device_id: 1,
            kind: KIND_TRACK.into(),
            track_id: Some(1),
            persistent_id: None,
            device_path: path.into(),
            object_id: None,
            source_hash: Some(hash.into()),
            encoded_codec: codec.into(),
            size_bytes: 100,
        }
    }

    #[test]
    fn a_new_track_is_an_add() {
        let plan = diff(&[desired_at("/Music/a.flac", "h1")], &[]);
        assert_eq!(plan.adds.len(), 1);
        assert_eq!(plan.bytes_out, 100);
    }

    #[test]
    fn an_identical_hash_and_codec_is_unchanged() {
        let plan = diff(
            &[desired_at("/Music/a.flac", "h1")],
            &[existing_at(1, "/Music/a.flac", "h1", "copy:flac")],
        );
        assert_eq!(plan.unchanged, 1);
        assert!(plan.adds.is_empty() && plan.replaces.is_empty());
        assert_eq!(plan.bytes_out, 0, "an unchanged track transfers nothing");
    }

    #[test]
    fn a_changed_hash_is_a_replace_carrying_the_row_id() {
        let plan = diff(
            &[desired_at("/Music/a.flac", "h2")],
            &[existing_at(7, "/Music/a.flac", "h1", "copy:flac")],
        );
        assert_eq!(plan.replaces.len(), 1);
        assert_eq!(
            plan.replaces[0].0, 7,
            "the row id lets the engine update in place"
        );
    }

    #[test]
    fn a_changed_codec_is_a_replace_even_at_the_same_hash() {
        let plan = diff(
            &[desired_codec("/Music/a.flac", "h1", "flac")],
            &[existing_at(7, "/Music/a.flac", "h1", "copy:alac")],
        );
        assert_eq!(
            plan.replaces.len(),
            1,
            "changing the transcode policy must re-push"
        );
    }

    #[test]
    fn an_unhashed_library_is_stable_across_runs() {
        // The iTunes ITL importer never writes `file_hash`, so this is
        // the ordinary case for a synced library, not an edge case.
        let mut want = desired_at("/Music/a.flac", "unused");
        want.source_hash = None;
        let mut have = existing_at(1, "/Music/a.flac", "unused", "copy:flac");
        have.source_hash = None;

        let plan = diff(&[want], &[have]);

        assert_eq!(plan.unchanged, 1, "an unhashed track must not re-push");
        assert_eq!(plan.bytes_out, 0);
    }

    #[test]
    fn an_unhashed_track_whose_size_changed_is_replaced() {
        let mut want = desired_at("/Music/a.flac", "unused");
        want.source_hash = None;
        want.size_bytes = 200;
        let mut have = existing_at(1, "/Music/a.flac", "unused", "copy:flac");
        have.source_hash = None;

        let plan = diff(&[want], &[have]);

        assert_eq!(plan.replaces.len(), 1, "size is the fallback signal");
    }

    #[test]
    fn an_unknown_hash_forces_a_replace() {
        let mut want = desired_at("/Music/a.flac", "h1");
        want.source_hash = None;
        let plan = diff(
            &[want],
            &[existing_at(7, "/Music/a.flac", "h1", "copy:flac")],
        );
        assert_eq!(plan.replaces.len(), 1);
    }

    #[test]
    fn a_dropped_track_is_an_orphan() {
        let plan = diff(
            &[],
            &[existing_at(3, "/Music/gone.flac", "h1", "copy:flac")],
        );
        assert_eq!(plan.orphans.len(), 1);
        assert_eq!(plan.orphans[0].id, 3);
    }

    #[test]
    fn a_track_moved_to_a_new_path_is_an_add_plus_an_orphan() {
        let plan = diff(
            &[desired_at("/Music/new.flac", "h1")],
            &[existing_at(3, "/Music/old.flac", "h1", "copy:flac")],
        );
        assert_eq!(plan.adds.len(), 1);
        assert_eq!(plan.orphans.len(), 1);
    }

    #[test]
    fn playlist_rows_are_never_orphaned_by_track_diffing() {
        let mut pl = existing_at(4, "/Music/Playlists/a.m3u8", "h", "m3u8");
        pl.kind = KIND_PLAYLIST.into();
        let plan = diff(&[], &[pl]);
        assert!(
            plan.orphans.is_empty(),
            "playlists are diffed separately, after uploads settle"
        );
    }

    #[test]
    fn bytes_out_sums_adds_and_replaces_only() {
        let plan = diff(
            &[
                desired_at("/Music/a.flac", "h1"),
                desired_at("/Music/b.flac", "h2"),
                desired_at("/Music/c.flac", "h3"),
            ],
            &[
                existing_at(1, "/Music/a.flac", "h1", "copy:flac"),
                existing_at(2, "/Music/b.flac", "old", "copy:flac"),
            ],
        );
        assert_eq!(plan.unchanged, 1);
        assert_eq!(plan.replaces.len(), 1);
        assert_eq!(plan.adds.len(), 1);
        assert_eq!(plan.bytes_out, 200);
    }
}
