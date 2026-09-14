//! Step three: regenerate the sidebar as one folder per umbrella genre,
//! each holding an "All <Genre>" playlist and one smart playlist per
//! artist.

use super::map::GenreMap;
use super::resolve::is_non_artist;
use super::taxonomy::Umbrella;
use crate::db::playlists::{
    self, create_generated, ids_with_marker_prefix, synced_smart_and_folder_ids, PlaylistKind,
};
use crate::db::smart::{Condition, ConditionGroup, LeafCondition, Op, SmartRule, Value};
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::BTreeMap;

/// `persistent_id` prefix on every row this module creates.
pub const MARKER_PREFIX: &str = "tuxtunes-genres:";

#[derive(Debug, Clone, Copy)]
pub struct RebuildOpts {
    /// Artists with fewer tracks get no playlist of their own.
    pub min_tracks: u64,
    /// Delete sync-sourced smart playlists and folders (tombstoned, so
    /// the next sync does not bring them back).
    pub replace_synced: bool,
    /// Report the tree that would be built, touch nothing.
    pub dry_run: bool,
}

impl Default for RebuildOpts {
    fn default() -> Self {
        Self {
            min_tracks: 5,
            replace_synced: true,
            dry_run: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RebuildSummary {
    pub deleted_generated: u64,
    pub deleted_synced: u64,
    pub folders: u64,
    pub playlists: u64,
    /// Artist playlists per folder name, in sidebar order.
    pub tree: BTreeMap<String, Vec<String>>,
}

fn leaf(field: &str, value: &str) -> Condition {
    Condition::Leaf(LeafCondition {
        field: field.to_string(),
        op: Op::Is,
        value: Value::Text(value.to_string()),
    })
}

/// Every track credited to the artist, as album artist or as artist.
pub fn artist_rule(key: &str) -> SmartRule {
    SmartRule {
        match_all: false,
        live_updating: true,
        limit: None,
        root: ConditionGroup {
            match_all: false,
            children: vec![leaf("album_artist", key), leaf("artist", key)],
        },
    }
}

/// Every track whose genre is one of the umbrella's specific genres.
pub fn umbrella_rule(genres: &[String]) -> SmartRule {
    SmartRule {
        match_all: false,
        live_updating: true,
        limit: None,
        root: ConditionGroup {
            match_all: false,
            children: genres.iter().map(|g| leaf("genre", g)).collect(),
        },
    }
}

pub fn folder_marker(u: Umbrella) -> String {
    format!("{MARKER_PREFIX}folder:{}", u.display_name())
}

pub fn all_marker(u: Umbrella) -> String {
    format!("{MARKER_PREFIX}all:{}", u.display_name())
}

pub fn artist_marker(key: &str) -> String {
    format!("{MARKER_PREFIX}artist:{key}")
}

/// The tree the map implies: umbrella → (specific genres, qualifying
/// artist keys), both sorted.
pub fn plan(map: &GenreMap, min_tracks: u64) -> BTreeMap<Umbrella, (Vec<String>, Vec<String>)> {
    let mut out: BTreeMap<Umbrella, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (key, entry) in &map.artists {
        if !entry.is_resolved() || is_non_artist(key) {
            continue;
        }
        let slot = out.entry(entry.umbrella()).or_default();
        if !slot.0.contains(&entry.genre) {
            slot.0.push(entry.genre.clone());
        }
        if entry.track_count >= min_tracks {
            slot.1.push(key.clone());
        }
    }
    out.retain(|_, (_, artists)| !artists.is_empty());
    for (genres, artists) in out.values_mut() {
        genres.sort();
        artists.sort_by_key(|a| a.to_lowercase());
    }
    out
}

pub async fn rebuild(
    engine: &SqliteRawEngine,
    map: &GenreMap,
    opts: RebuildOpts,
) -> anyhow::Result<RebuildSummary> {
    let planned = plan(map, opts.min_tracks);
    let mut summary = RebuildSummary::default();
    for (u, (_, artists)) in &planned {
        summary
            .tree
            .insert(u.display_name().to_string(), artists.clone());
    }
    let generated = ids_with_marker_prefix(engine, MARKER_PREFIX).await?;
    let synced = if opts.replace_synced {
        synced_smart_and_folder_ids(engine).await?
    } else {
        Vec::new()
    };
    summary.deleted_generated = generated.len() as u64;
    summary.deleted_synced = synced.len() as u64;
    summary.folders = planned.len() as u64;
    summary.playlists = planned
        .values()
        .map(|(_, artists)| artists.len() as u64 + 1)
        .sum();
    if opts.dry_run {
        return Ok(summary);
    }

    // Children before folders is not required (FK is SET NULL) but
    // deleting in reverse id order keeps folders last anyway.
    for id in generated.iter().rev().chain(synced.iter().rev()) {
        playlists::delete(engine, *id).await?;
    }

    for (u, (genres, artists)) in &planned {
        let folder = create_generated(
            engine,
            u.display_name(),
            PlaylistKind::Folder,
            None,
            None,
            &folder_marker(*u),
        )
        .await?;
        let all_rule = umbrella_rule(genres);
        let all_id = create_generated(
            engine,
            &format!("All {}", u.display_name()),
            PlaylistKind::Smart,
            Some(folder),
            Some(&serde_json::to_string(&all_rule)?),
            &all_marker(*u),
        )
        .await?;
        cache_count(engine, all_id, &all_rule).await?;
        for key in artists {
            let rule = artist_rule(key);
            let id = create_generated(
                engine,
                key,
                PlaylistKind::Smart,
                Some(folder),
                Some(&serde_json::to_string(&rule)?),
                &artist_marker(key),
            )
            .await?;
            cache_count(engine, id, &rule).await?;
        }
    }
    Ok(summary)
}

async fn cache_count(engine: &SqliteRawEngine, id: i64, rule: &SmartRule) -> anyhow::Result<()> {
    let n = crate::db::smart::preview_count(engine, rule).await?;
    playlists::set_cached_count(engine, id, n).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::playlists::{
        create_regular, create_smart, list_all, tombstoned_pids, upsert, PlaylistUpsert,
    };
    use crate::db::Db;
    use crate::library::genres::map::{ArtistGenre, Source};
    use prax_query::filter::FilterValue as FV;

    async fn tmp() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        db
    }

    async fn insert_track(db: &Db, artist: &str, album_artist: Option<&str>, genre: &str) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, artist, album_artist, genre, duration_ms, size_bytes, \
                 file_path, playlist_ids) VALUES (?, ?, ?, ?, 1000, 0, ?, '[]')",
                &[
                    FV::String(format!("t{n}")),
                    FV::String(artist.to_string()),
                    album_artist
                        .map(|s| FV::String(s.to_string()))
                        .unwrap_or(FV::Null),
                    FV::String(genre.to_string()),
                    FV::String(format!("/tmp/r{n}.flac")),
                ],
            )
            .await
            .unwrap();
    }

    fn map() -> GenreMap {
        let mut m = GenreMap::default();
        m.artists.insert(
            "Zeal".into(),
            ArtistGenre::new("Metalcore", Source::Musicbrainz, 6),
        );
        m.artists.insert(
            "abba".into(),
            ArtistGenre::new("Death Metal", Source::Tags, 5),
        );
        m.artists
            .insert("Tiny".into(), ArtistGenre::new("Djent", Source::Tags, 2));
        m.artists.insert(
            "Daft".into(),
            ArtistGenre::new("House", Source::Musicbrainz, 9),
        );
        m.artists.insert(
            "Various Artists".into(),
            ArtistGenre::new("Rock", Source::Tags, 50),
        );
        m.artists
            .insert("Nobody".into(), ArtistGenre::unresolved(40));
        m
    }

    #[test]
    fn rules_have_the_expected_shape() {
        let r = artist_rule("Daft");
        assert!(!r.match_all && r.live_updating && r.limit.is_none());
        assert_eq!(r.root.children.len(), 2);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""field":"album_artist","op":"is","value":"Daft""#));
        assert!(json.contains(r#""field":"artist","op":"is","value":"Daft""#));
        let u = umbrella_rule(&["Djent".into(), "Metalcore".into()]);
        assert_eq!(u.root.children.len(), 2);
    }

    #[test]
    fn plan_groups_sorts_and_applies_threshold() {
        let p = plan(&map(), 5);
        let metal = &p[&Umbrella::Metal];
        assert_eq!(metal.0, vec!["Death Metal", "Djent", "Metalcore"]);
        assert_eq!(metal.1, vec!["abba", "Zeal"]);
        assert_eq!(p[&Umbrella::Electronic].1, vec!["Daft"]);
        assert!(
            !p.contains_key(&Umbrella::Rock),
            "Various Artists never gets a playlist"
        );
        assert!(!p.contains_key(&Umbrella::Other));
        let all = plan(&map(), 1);
        assert_eq!(all[&Umbrella::Metal].1, vec!["abba", "Tiny", "Zeal"]);
    }

    #[tokio::test]
    async fn rebuild_replaces_synced_tree_keeps_user_rows_and_is_idempotent() {
        let db = tmp().await;
        for _ in 0..3 {
            insert_track(&db, "Zeal", None, "Metalcore").await;
        }
        insert_track(&db, "Zeal feat. X", Some("Zeal"), "Metalcore").await;
        insert_track(&db, "Daft", None, "House").await;

        let mk = |pid: u64, name: &'static str, kind: PlaylistKind| PlaylistUpsert {
            persistent_id: pid,
            sync_source_id: 1,
            name,
            kind,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[],
            smart_rule_json: None,
        };
        let s_folder = upsert(&db.engine, &mk(1, "Metal (old)", PlaylistKind::Folder))
            .await
            .unwrap();
        let s_smart = upsert(&db.engine, &mk(2, "Zeal (old)", PlaylistKind::Smart))
            .await
            .unwrap();
        let s_reg = upsert(&db.engine, &mk(3, "Road trip", PlaylistKind::Regular))
            .await
            .unwrap();
        let u_reg = create_regular(&db.engine, "Mine", None).await.unwrap();
        let u_smart = create_smart(&db.engine, "My smart", "{}").await.unwrap();

        let dry = rebuild(
            &db.engine,
            &map(),
            RebuildOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(dry.deleted_synced, 2);
        assert_eq!(dry.folders, 2);
        assert_eq!(dry.playlists, 5);
        assert_eq!(
            list_all(&db.engine).await.unwrap().len(),
            5,
            "dry run touched nothing"
        );

        let s = rebuild(&db.engine, &map(), RebuildOpts::default())
            .await
            .unwrap();
        assert_eq!(s.deleted_generated, 0);
        assert_eq!(s.deleted_synced, 2);
        assert_eq!(s.tree["Metal"], vec!["abba", "Zeal"]);
        assert_eq!(s.tree["Electronic"], vec!["Daft"]);

        let rows = list_all(&db.engine).await.unwrap();
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert!(!ids.contains(&s_folder) && !ids.contains(&s_smart));
        assert!(ids.contains(&s_reg) && ids.contains(&u_reg) && ids.contains(&u_smart));
        let dead = tombstoned_pids(&db.engine, 1).await.unwrap();
        assert!(dead.contains(&1) && dead.contains(&2) && !dead.contains(&3));

        let by_name = |n: &str| rows.iter().find(|r| r.name == n).cloned().unwrap();
        let metal = by_name("Metal");
        assert_eq!(metal.kind, "folder");
        assert_eq!(metal.parent_id, None);
        assert_eq!(metal.sync_source_id, None);
        let all_metal = by_name("All Metal");
        assert_eq!(all_metal.parent_id, Some(metal.id));
        assert_eq!(all_metal.cached_track_count, Some(4));
        let zeal = by_name("Zeal");
        assert_eq!(zeal.parent_id, Some(metal.id));
        assert_eq!(
            zeal.cached_track_count,
            Some(4),
            "album-artist credit counts too"
        );
        assert_eq!(by_name("Daft").parent_id, Some(by_name("Electronic").id));
        assert!(all_metal.sort_order < zeal.sort_order);
        assert!(by_name("abba").sort_order < zeal.sort_order);
        assert!(
            metal.sort_order < by_name("Electronic").sort_order,
            "folders in umbrella order"
        );
        assert_eq!(rows.len(), 3 + 7);

        let again = rebuild(&db.engine, &map(), RebuildOpts::default())
            .await
            .unwrap();
        assert_eq!(again.deleted_generated, 7);
        assert_eq!(again.deleted_synced, 0);
        assert_eq!(list_all(&db.engine).await.unwrap().len(), 3 + 7);

        let kept = rebuild(
            &db.engine,
            &map(),
            RebuildOpts {
                replace_synced: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(kept.deleted_synced, 0);
    }
}
