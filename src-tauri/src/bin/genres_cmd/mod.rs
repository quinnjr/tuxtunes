//! `tuxtunes-cli genres …`: resolve artist genres from MusicBrainz,
//! apply them to tracks and files, and rebuild the sidebar as one
//! folder per genre. Kept beside `import_cmd` so the CLI's dispatch
//! `match` stays thin.

use std::path::Path;
use tuxtunes::library::genres::apply::{apply, ApplyOpts};
use tuxtunes::library::genres::map::{GenreMap, Source};
use tuxtunes::library::genres::musicbrainz::MusicBrainz;
use tuxtunes::library::genres::rebuild::{rebuild, RebuildOpts};
use tuxtunes::library::genres::resolve::{resolve, ResolveOpts};

#[derive(clap::Subcommand, Debug)]
pub enum GenresCommand {
    /// Look up every artist on MusicBrainz and record its genre in
    /// genre_map.json beside the database. Re-running only queries
    /// artists not yet resolved; entries marked "manual" are never
    /// overwritten.
    Resolve {
        /// Re-query artists already resolved from MusicBrainz or tags.
        #[arg(long)]
        refresh: bool,
    },
    /// Write the resolved genres onto every track (database row and
    /// the file's own genre tag). Rows get their genre locked so a
    /// later sync does not revert it; other fields stay sync-owned.
    Apply {
        /// Update database rows only; leave file tags alone.
        #[arg(long)]
        no_tags: bool,
        /// Also revisit the file tag of every track whose genre is
        /// already locked, to finish a tag phase that was interrupted.
        #[arg(long, conflicts_with = "no_tags")]
        retag: bool,
        /// Report what would change without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate the playlist tree: one folder per genre holding an
    /// "All <Genre>" playlist and one smart playlist per artist.
    /// Replaces the smart playlists and folders that came from a sync
    /// source (they are tombstoned so a sync will not bring them back);
    /// regular playlists and anything you made yourself are kept.
    Rebuild {
        /// Artists with fewer tracks get no playlist of their own.
        #[arg(long, default_value_t = 5)]
        min_tracks: u64,
        /// Keep sync-sourced smart playlists and folders instead of
        /// replacing them.
        #[arg(long)]
        keep_synced: bool,
        /// Print the tree that would be built without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(db: &tuxtunes::db::Db, db_path: &Path, cmd: GenresCommand) -> anyhow::Result<()> {
    let map_path = GenreMap::path_for(db_path);
    match cmd {
        GenresCommand::Resolve { refresh } => {
            let mut map = GenreMap::load(&map_path)?.unwrap_or_default();
            let genre_cache = map_path.with_file_name("musicbrainz_genres.txt");
            let genres = MusicBrainz::genre_list(&genre_cache).await?;
            let mb = MusicBrainz::new(genres)?;
            let summary = resolve(
                &db.engine,
                &mb,
                &mut map,
                ResolveOpts { refresh },
                |m| m.save(&map_path).map_err(Into::into),
                |key, done, total| eprintln!("[{done}/{total}] {key}"),
            )
            .await?;
            println!(
                "looked_up={} musicbrainz={} tags={} unresolved={} skipped={} map={}",
                summary.looked_up,
                summary.musicbrainz,
                summary.tags,
                summary.unresolved,
                summary.skipped,
                map_path.display()
            );
            let unresolved: Vec<&String> = map
                .artists
                .iter()
                .filter(|(_, e)| e.source == Source::Unresolved && e.track_count >= 5)
                .map(|(k, _)| k)
                .collect();
            if !unresolved.is_empty() {
                eprintln!(
                    "{} artists with 5+ tracks are unresolved; set their genre in {} with \
                     \"source\": \"manual\" to place them:",
                    unresolved.len(),
                    map_path.display()
                );
                for k in unresolved {
                    eprintln!("  {k}");
                }
            }
            Ok(())
        }
        GenresCommand::Apply {
            no_tags,
            retag,
            dry_run,
        } => {
            let map = load_required(&map_path)?;
            let summary = apply(
                &db.engine,
                &map,
                ApplyOpts {
                    write_tags: !no_tags,
                    retag,
                    dry_run,
                },
                |done, total| eprintln!("tags {done}/{total}"),
            )
            .await?;
            for (genre, n) in &summary.by_genre {
                println!("{n}\t{genre}");
            }
            println!(
                "planned={} db_updated={} tags_written={} tags_missing={} tags_failed={}{}",
                summary.planned,
                summary.db_updated,
                summary.tags_written,
                summary.tags_skipped_missing,
                summary.tags_failed.len(),
                if dry_run { " (dry run)" } else { "" }
            );
            for f in &summary.tags_failed {
                eprintln!("failed: {f}");
            }
            if !summary.tags_failed.is_empty() {
                anyhow::bail!("{} file(s) failed to tag", summary.tags_failed.len());
            }
            Ok(())
        }
        GenresCommand::Rebuild {
            min_tracks,
            keep_synced,
            dry_run,
        } => {
            let map = load_required(&map_path)?;
            let summary = rebuild(
                &db.engine,
                &map,
                RebuildOpts {
                    min_tracks,
                    replace_synced: !keep_synced,
                    dry_run,
                },
            )
            .await?;
            for (folder, artists) in &summary.tree {
                println!("{folder} ({} artists)", artists.len());
                for a in artists {
                    println!("  {a}");
                }
            }
            if !summary.unfoldered.is_empty() {
                eprintln!(
                    "{} regular playlist(s) lose their synced folder and move to the top level:",
                    summary.unfoldered.len()
                );
                for name in &summary.unfoldered {
                    eprintln!("  {name}");
                }
            }
            println!(
                "deleted_generated={} deleted_synced={} folders={} playlists={}{}",
                summary.deleted_generated,
                summary.deleted_synced,
                summary.folders,
                summary.playlists,
                if dry_run { " (dry run)" } else { "" }
            );
            Ok(())
        }
    }
}

fn load_required(map_path: &Path) -> anyhow::Result<GenreMap> {
    GenreMap::load(map_path)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no genre map at {}; run `tuxtunes-cli genres resolve` first",
            map_path.display()
        )
    })
}
