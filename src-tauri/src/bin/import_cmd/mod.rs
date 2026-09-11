//! Headless equivalent of the GUI's Add Folder: probe audio files into
//! the library, then copy every new file under the managed root.
//!
//! Kept beside (not inside) `tuxtunes-cli.rs` so the dispatch `match`
//! stays thin as subcommands accumulate. Dedup policy is not
//! reimplemented here — single files go through
//! [`tuxtunes::library::ingest::ensure_track`] and directories through
//! [`tuxtunes::library::ingest::add_folder`], the same helpers the GUI
//! uses.

use std::path::{Path, PathBuf};

/// What `import` did, across every path it was given.
#[derive(Debug, Default)]
pub struct ImportSummary {
    /// Files newly added to the library and copied under its root.
    /// A copy failure decrements this again and lands in `failed`
    /// instead, so `added` only ever counts landed copies.
    pub added: u64,
    /// Files the library already referenced; left untouched.
    pub skipped: u64,
    /// Human-readable per-path failures; echoed to stderr by the caller.
    pub failed: Vec<String>,
}

/// One track waiting for its managed copy.
struct PendingImport {
    id: i64,
    source: PathBuf,
}

impl ImportSummary {
    fn fail(&mut self, path: &Path, msg: String) {
        self.failed.push(format!("{}: {msg}", path.display()));
    }

    /// Fold an `add_folder` result in. The rows to copy do not come
    /// from here — see [`collect_one`].
    fn absorb(&mut self, folder: tuxtunes::library::ingest::AddFolderSummary) {
        self.added += folder.added;
        self.skipped += folder.skipped;
        self.failed.extend(folder.failed);
    }
}

/// Import files and directories into the library, then copy every
/// newly added file under the managed library root — the headless
/// equivalent of the GUI's Add Folder (probe, insert, copy-on-add).
/// Per-file failures are recorded, not fatal; a database error stops
/// the walk but keeps what already landed.
pub async fn run_import(
    db: &tuxtunes::db::Db,
    paths: &[std::path::PathBuf],
) -> anyhow::Result<ImportSummary> {
    let mut summary = ImportSummary::default();
    let mut pending = Vec::new();
    for path in paths {
        collect_one(&db.engine, path, &mut summary, &mut pending).await;
    }
    copy_pending(&db.engine, &mut summary, pending).await;
    Ok(summary)
}

/// Pin one CLI argument to the file it names, insert its tracks, and
/// queue their copies. Canonicalizing before any DB comparison or
/// insert keeps dedup (which is exact-string matching) from turning
/// `./x/a.wav` and `x/a.wav` into two rows — and narrows the
/// check-then-walk window for a swapped top-level symlink to the
/// resolved target itself.
async fn collect_one(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    path: &Path,
    summary: &mut ImportSummary,
    pending: &mut Vec<PendingImport>,
) {
    use tuxtunes::library::ingest as lib_ingest;

    let canon = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) if !path.exists() => {
            summary.fail(path, "no such file or directory".to_string());
            return;
        }
        Err(e) => {
            summary.fail(path, format!("{e:#}"));
            return;
        }
    };

    if canon.is_dir() {
        match lib_ingest::add_folder(engine, &canon).await {
            Ok(folder) => {
                summary.absorb(folder);
                // The rows to copy come from the sweep, not
                // `added_tracks`: it also re-queues tracks stranded by
                // an earlier run whose copy failed after the insert
                // landed — otherwise the re-run would count them as
                // done while they still sit outside the root.
                match lib_ingest::pending_copies(engine, &canon).await {
                    Ok(sweep) => pending.extend(
                        sweep
                            .into_iter()
                            .map(|(id, source)| PendingImport { id, source }),
                    ),
                    Err(e) => summary.fail(&canon, format!("{e:#}")),
                }
            }
            Err(e) => summary.fail(&canon, format!("{e:#}")),
        }
        return;
    }

    match lib_ingest::ensure_track(engine, &canon).await {
        Ok(Some(id)) => {
            summary.added += 1;
            pending.push(PendingImport { id, source: canon });
        }
        Ok(None) => match pending_recopy(engine, &canon).await {
            // Known row whose file already landed: nothing to do.
            Ok(None) => summary.skipped += 1,
            // Known row stranded outside the root: finish its copy.
            Ok(Some(p)) => pending.push(p),
            Err(e) => summary.fail(&canon, format!("{e:#}")),
        },
        Err(e) => summary.fail(&canon, format!("{e:#}")),
    }
}

/// Re-queue the copy for a known row whose file never landed under the
/// managed root, or `None` when there is nothing to finish. Reads the
/// row's current `file_path` (not the typed spelling) as the source.
async fn pending_recopy(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    canon: &Path,
) -> anyhow::Result<Option<PendingImport>> {
    use tuxtunes::library::ingest as lib_ingest;

    let Some(id) = lib_ingest::track_id_for_path(engine, canon).await? else {
        // Deleted between the dedup check and now; skipping a gone row
        // is harmless.
        return Ok(None);
    };
    let row = tuxtunes::db::tracks::get(engine, id).await?;
    let root = tuxtunes::db::preferences::get_library_root(engine).await?;
    if tuxtunes::fs::path::is_under(&root, Path::new(&row.file_path)) {
        return Ok(None);
    }
    Ok(Some(PendingImport {
        id,
        source: PathBuf::from(row.file_path),
    }))
}

/// Copy every queued track under the managed root. A copy failure
/// un-counts the insert and records the file; only a source that is
/// actually gone marks the row missing — anything else leaves a row
/// that still plays from where it is (the worker's policy, mirrored).
async fn copy_pending(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    summary: &mut ImportSummary,
    pending: Vec<PendingImport>,
) {
    for p in pending {
        if let Err(e) = tuxtunes::fs::ingest::ingest_one_headless(engine, p.id, &p.source).await {
            summary.added = summary.added.saturating_sub(1);
            summary.fail(&p.source, format!("{e:#}"));
            if !p.source.exists() {
                if let Err(e) = tuxtunes::db::tracks::mark_missing_source(engine, p.id).await {
                    summary.fail(&p.source, format!("failed to mark missing: {e:#}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic WAV that lofty will happily parse.
    fn write_minimal_wav(path: &std::path::Path) {
        let header: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x25, 0x00, 0x00, 0x00, b'W', b'A', b'V', b'E', b'f', b'm',
            b't', b' ', 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x40, 0x1f, 0x00, 0x00,
            0x40, 0x1f, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, b'd', b'a', b't', b'a', 0x01, 0x00,
            0x00, 0x00, 0x80,
        ];
        std::fs::write(path, header).unwrap();
    }

    async fn test_db(tmp: &std::path::Path) -> (tuxtunes::db::Db, PathBuf) {
        let db = tuxtunes::db::Db::open(&tmp.join("t.db")).await.unwrap();
        let root = tmp.join("managed");
        tuxtunes::db::preferences::set_library_root(&db.engine, &root)
            .await
            .unwrap();
        (db, root)
    }

    #[tokio::test]
    async fn import_folder_ingests_files_and_copies_into_library_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root) = test_db(tmp.path()).await;

        let incoming = tmp.path().join("incoming");
        std::fs::create_dir_all(&incoming).unwrap();
        write_minimal_wav(&incoming.join("a.wav"));
        write_minimal_wav(&incoming.join("b.wav"));

        let summary = run_import(&db, std::slice::from_ref(&incoming))
            .await
            .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.skipped, 0);
        assert!(summary.failed.is_empty());

        // Re-running is a no-op: the same sources are recognised.
        let again = run_import(&db, &[incoming]).await.unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.skipped, 2);

        // Every row now lives under the managed root.
        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let prefix = root.display().to_string();
        assert!(rows.iter().all(|r| r.file_path.starts_with(&prefix)));
    }

    #[tokio::test]
    async fn import_missing_path_records_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, _) = test_db(tmp.path()).await;

        let summary = run_import(&db, &[tmp.path().join("gone")]).await.unwrap();
        assert_eq!(summary.added, 0);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.failed.len(), 1);
        assert!(summary.failed[0].contains("no such file"));
    }

    #[tokio::test]
    async fn import_single_file_adds_then_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root) = test_db(tmp.path()).await;

        let file = tmp.path().join("solo.wav");
        write_minimal_wav(&file);

        let first = run_import(&db, std::slice::from_ref(&file)).await.unwrap();
        assert_eq!(first.added, 1);
        assert!(first.failed.is_empty());

        let second = run_import(&db, std::slice::from_ref(&file)).await.unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.skipped, 1);
        assert!(second.failed.is_empty());

        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let prefix = root.display().to_string();
        assert!(rows[0].file_path.starts_with(&prefix));
    }

    #[tokio::test]
    async fn import_non_canonical_spelling_does_not_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, _) = test_db(tmp.path()).await;

        let file = tmp.path().join("a.wav");
        write_minimal_wav(&file);
        // Same file through a `..` spelling: must resolve, not double.
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let dotdot = tmp.path().join("sub").join("..").join("a.wav");

        let first = run_import(&db, std::slice::from_ref(&file)).await.unwrap();
        assert_eq!(first.added, 1);
        let second = run_import(&db, std::slice::from_ref(&dotdot))
            .await
            .unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.skipped, 1);

        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    /// chmod-based failure injection below assumes a non-root user (the
    /// developer workstation and CI); root ignores permission bits.
    #[tokio::test]
    async fn import_copy_failure_keeps_playable_row() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let (db, root) = test_db(tmp.path()).await;

        let incoming = tmp.path().join("incoming");
        std::fs::create_dir_all(&incoming).unwrap();
        write_minimal_wav(&incoming.join("a.wav"));

        // Read-only library root: the insert lands, the copy fails.
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let summary = run_import(&db, std::slice::from_ref(&incoming))
            .await
            .unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(summary.added, 0, "failed copies must not count as added");
        assert_eq!(summary.failed.len(), 1);
        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].import_status, "ok", "row still plays from source");

        // Healing: with the root writable again the re-run finishes it.
        let healed = run_import(&db, std::slice::from_ref(&incoming))
            .await
            .unwrap();
        assert!(healed.failed.is_empty());
        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        let prefix = root.display().to_string();
        assert!(rows[0].file_path.starts_with(&prefix));
    }

    #[tokio::test]
    async fn import_copy_failure_marks_gone_source() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let (db, root) = test_db(tmp.path()).await;

        let file = tmp.path().join("a.wav");
        write_minimal_wav(&file);

        // Interleave: collect the insert, then lose the source and the
        // writable root before the copy runs.
        let mut summary = ImportSummary::default();
        let mut pending = Vec::new();
        collect_one(&db.engine, &file, &mut summary, &mut pending).await;
        assert_eq!(pending.len(), 1);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        copy_pending(&db.engine, &mut summary, pending).await;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(summary.failed.len(), 1);
        let rows = tuxtunes::db::tracks::list(&db.engine, 100, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].import_status, "missing_source");
    }
}
