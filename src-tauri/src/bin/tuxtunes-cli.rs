use clap::Parser;
use std::path::PathBuf;

mod import_cmd;

#[derive(Parser, Debug)]
#[command(
    name = "tuxtunes-cli",
    about = "Manage the TuxTunes library: import audio, manage iTunes .itl sync sources"
)]
struct Cli {
    /// Path to the library database (defaults to the desktop app's DB).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Manage sync sources.
    #[command(subcommand)]
    Source(SourceCommand),
    /// Run sync reconciliation.
    #[command(subcommand)]
    Sync(SyncCommand),
    /// Add audio files to the library. Each path is a file or a
    /// directory (directories are walked recursively); added files are
    /// copied under the managed library root, exactly like the GUI's
    /// Add Folder. Re-running is a no-op for files already imported.
    Import {
        /// Files or directories to import.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Trash the source files that importing copied under the managed
    /// library root. Each original is hash-verified against its copy
    /// first and goes to the system trash, never unlink — the GUI's
    /// Reclaim Originals, headless.
    Reclaim,
}

#[derive(clap::Subcommand, Debug)]
enum SourceCommand {
    /// List configured sync sources.
    List,
    /// Add a sync source pointing at an iTunes .itl file.
    Add {
        /// Display name for the source.
        #[arg(long)]
        name: String,
        /// Path to the iTunes .itl file.
        itl_path: std::path::PathBuf,
        /// Path remap, repeatable: --map FROM=TO
        #[arg(long = "map")]
        map: Vec<String>,
    },
    /// Remove a sync source by id.
    Remove { id: i64 },
}

#[derive(clap::Subcommand, Debug)]
enum SyncCommand {
    /// Reconcile a source by id, or all sources with --all.
    Run {
        /// Source id to reconcile.
        id: Option<i64>,
        /// Reconcile every configured source.
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },
}

struct CliObserver;

impl tuxtunes::sync::observer::SyncObserver for CliObserver {
    fn progress(&self, ev: &tuxtunes::sync::events::SyncProgress) {
        eprintln!(
            "[{:?}] {} ({}/{})",
            ev.phase, ev.message, ev.current, ev.total
        );
    }
    fn warning(&self, ev: &tuxtunes::sync::events::SyncWarning) {
        eprintln!("warning: {:?}: {}", ev.kind, ev.detail);
    }
    fn complete(&self, _ev: &tuxtunes::sync::events::SyncComplete) {}
    fn failed(&self, ev: &tuxtunes::sync::events::SyncFailed) {
        eprintln!("failed: source {}: {}", ev.source_id, ev.error);
    }
}

fn summary_line(c: &tuxtunes::sync::events::SyncComplete) -> String {
    format!(
        "source {}: tracks +{} ~{} -{}, playlists +{} ~{} -{}",
        c.source_id,
        c.inserted_tracks,
        c.updated_tracks,
        c.deleted_tracks,
        c.inserted_playlists,
        c.updated_playlists,
        c.deleted_playlists,
    )
}

fn parse_mapping(s: &str) -> Result<tuxtunes::sync::path_map::PathMapping, String> {
    let (from, to) = s
        .split_once('=')
        .ok_or_else(|| format!("expected FROM=TO, got {s:?}"))?;
    if from.is_empty() || to.is_empty() {
        return Err(format!("FROM and TO must be non-empty in {s:?}"));
    }
    Ok(tuxtunes::sync::path_map::PathMapping {
        from: from.to_string(),
        to: to.to_string(),
    })
}

fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .expect("a platform data dir")
        .join("dev.quinnjr.tuxtunes")
        .join("tuxtunes.db")
}

fn main() -> std::process::ExitCode {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[tokio::main]
async fn run_async(cli: Cli) -> anyhow::Result<()> {
    let db_path = cli.db.unwrap_or_else(default_db_path);
    let db = tuxtunes::db::Db::open(&db_path).await?;
    match cli.command {
        Command::Source(SourceCommand::List) => {
            let sources = tuxtunes::db::sync_sources::list(&db.engine).await?;
            if sources.is_empty() {
                println!("(no sync sources)");
            }
            for s in sources {
                println!(
                    "{}\t{}\t{}\tlast_sync={}",
                    s.id,
                    s.name,
                    s.source_path,
                    s.last_sync_at.as_deref().unwrap_or("never")
                );
            }
            Ok(())
        }
        Command::Source(SourceCommand::Add {
            name,
            itl_path,
            map,
        }) => {
            let mappings = map
                .iter()
                .map(|m| parse_mapping(m))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| anyhow::anyhow!(e))?;
            let id = tuxtunes::db::sync_sources::insert(
                &db.engine,
                &name,
                &itl_path.display().to_string(),
                &mappings,
                &Default::default(),
                false,
            )
            .await?;
            println!("added source {id}");
            Ok(())
        }
        Command::Source(SourceCommand::Remove { id }) => {
            tuxtunes::db::sync_sources::remove(&db.engine, id).await?;
            println!("removed source {id}");
            Ok(())
        }
        Command::Sync(SyncCommand::Run { id, all }) => {
            let ids: Vec<i64> = if all {
                tuxtunes::db::sync_sources::list(&db.engine)
                    .await?
                    .into_iter()
                    .map(|s| s.id)
                    .collect()
            } else {
                vec![id.ok_or_else(|| anyhow::anyhow!("provide a source id or --all"))?]
            };
            if ids.is_empty() {
                println!("(no sync sources to run)");
                return Ok(());
            }
            let obs = CliObserver;
            // CLI runs reconcile headless: no file ingest (fs = None) and no
            // import-log file (the per-run log + tailer is GUI-only), so the
            // log sink is a no-op. Progress still streams to stderr via obs.
            let log_sink = |_level: tuxtunes::sync::import_log::LogLevel, _msg: &str| {};
            let db = std::sync::Arc::new(db);
            let mut failures = 0u32;
            for id in ids {
                match tuxtunes::sync::worker::reconcile_source(&db, None, &obs, &log_sink, id).await
                {
                    Ok(complete) => println!("{}", summary_line(&complete)),
                    Err(e) => {
                        eprintln!("source {id} failed: {e:#}");
                        failures += 1;
                    }
                }
            }
            if failures > 0 {
                anyhow::bail!("{failures} source(s) failed");
            }
            Ok(())
        }
        Command::Import { paths } => {
            // clap's `required = true` rejects the empty case; an empty
            // slice here returns a zero summary by construction.
            let summary = import_cmd::run_import(&db, &paths).await?;
            println!(
                "added={} skipped={} failed={}",
                summary.added,
                summary.skipped,
                summary.failed.len()
            );
            for f in &summary.failed {
                eprintln!("failed: {f}");
            }
            if !summary.failed.is_empty() {
                anyhow::bail!("{} file(s) failed to import", summary.failed.len());
            }
            Ok(())
        }
        Command::Reclaim => {
            let stats = tuxtunes::fs::reclaim::reclaim_all_headless(&db.engine).await?;
            println!(
                "reclaimed={} bytes_freed={} skipped={} failed={}",
                stats.reclaimed, stats.bytes_freed, stats.skipped, stats.failed
            );
            if stats.failed > 0 {
                anyhow::bail!("{} file(s) failed to reclaim", stats.failed);
            }
            Ok(())
        }
    }
}

fn run() -> anyhow::Result<()> {
    run_async(Cli::parse())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn default_db_path_ends_with_app_db() {
        let p = default_db_path();
        assert!(p.ends_with("dev.quinnjr.tuxtunes/tuxtunes.db"), "got {p:?}");
    }

    #[test]
    fn parses_source_list_subcommand() {
        let cli = Cli::try_parse_from(["tuxtunes-cli", "source", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Source(SourceCommand::List)));
    }

    #[test]
    fn parse_mapping_splits_on_first_equals() {
        let m = parse_mapping("D:/=/mnt/music").unwrap();
        assert_eq!(m.from, "D:/");
        assert_eq!(m.to, "/mnt/music");
    }

    #[test]
    fn parse_mapping_rejects_missing_equals() {
        assert!(parse_mapping("nope").is_err());
    }

    #[test]
    fn parses_source_add_with_maps() {
        let cli = Cli::try_parse_from([
            "tuxtunes-cli",
            "source",
            "add",
            "--name",
            "Main",
            "--map",
            "D:/=/mnt",
            "lib.itl",
        ])
        .unwrap();
        match cli.command {
            Command::Source(SourceCommand::Add {
                name,
                itl_path,
                map,
            }) => {
                assert_eq!(name, "Main");
                assert_eq!(itl_path, std::path::PathBuf::from("lib.itl"));
                assert_eq!(map, vec!["D:/=/mnt".to_string()]);
            }
            _ => panic!("expected source add"),
        }
    }

    #[test]
    fn summary_line_reports_counts() {
        let c = tuxtunes::sync::events::SyncComplete {
            source_id: 7,
            inserted_tracks: 10,
            updated_tracks: 2,
            deleted_tracks: 1,
            inserted_playlists: 3,
            updated_playlists: 0,
            deleted_playlists: 0,
        };
        let line = summary_line(&c);
        assert!(line.contains("source 7"));
        assert!(line.contains("tracks +10 ~2 -1"));
        assert!(line.contains("playlists +3 ~0 -0"));
    }

    #[test]
    fn parses_sync_run_all() {
        let cli = Cli::try_parse_from(["tuxtunes-cli", "sync", "run", "--all"]).unwrap();
        match cli.command {
            Command::Sync(SyncCommand::Run { id, all }) => {
                assert!(all);
                assert!(id.is_none());
            }
            _ => panic!("expected sync run"),
        }
    }

    #[test]
    fn parse_mapping_rejects_empty_from_or_to() {
        assert!(parse_mapping("=/mnt/music").is_err());
        assert!(parse_mapping("D:/=").is_err());
    }

    #[test]
    fn parses_import_with_paths() {
        let cli =
            Cli::try_parse_from(["tuxtunes-cli", "import", "/music/a", "/music/b.flac"]).unwrap();
        match cli.command {
            Command::Import { paths } => {
                assert_eq!(
                    paths,
                    vec![PathBuf::from("/music/a"), PathBuf::from("/music/b.flac")]
                );
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn import_requires_at_least_one_path() {
        let cli = Cli::try_parse_from(["tuxtunes-cli", "import"]);
        assert!(cli.is_err());
    }

    #[test]
    fn parses_reclaim() {
        let cli = Cli::try_parse_from(["tuxtunes-cli", "reclaim"]).unwrap();
        assert!(matches!(cli.command, Command::Reclaim));
    }
}
