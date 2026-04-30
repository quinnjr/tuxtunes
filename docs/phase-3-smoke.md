# Phase 3 Smoke Test

Manual verification that the ITL import wizard performs an end-to-end sync against the user's real iTunes library.

## Pre-reqs

- `~/Music/iTunes/iTunes Library.itl` present.
- Media drive mounted at a known path (e.g., `/run/media/joseph/Local Disk/`).
- `itl-rs 0.2` on crates.io (pulled by `src-tauri/Cargo.toml`).
- `npm ci && npm run codegen && cd src-tauri && cargo build` all clean.

## Steps

1. `cd .worktrees/feature/phase-3-itl-import && npx tauri dev`
2. App opens. Click **Import iTunes library…** in the sidebar.
3. Wizard opens at "pick" step. Click **Pick .itl…**. Choose `~/Music/iTunes/iTunes Library.itl`. **Next**.
4. Wizard advances to "map". Verify default mappings (`D:/` → Linux mount, `C:/` → Linux mount). Adjust `to` paths to match your real mount. **Next**.
5. Wizard advances to "conflict". Leave defaults (prefer_source for rating/loved, last_write_wins for counts/dates). **Sync now**.
6. Wizard advances to "progress". Watch phase indicator move through decoding → applying_tracks → applying_playlists → finalizing.
7. On completion, a summary card shows +N/~M/-K for tracks and playlists. Some warnings are expected for DRM/missing files and cloud-only tracks (~18% of tracks in typical iTunes libraries have no local path).
8. Close the wizard. Switch the main content to **Tracks** view — all synced tracks should appear in the virtual list.
9. Click any imported track. Playback should start if the file path exists on disk.
10. Quit and reopen the app. Run the wizard again, pointing at the same `.itl`. Progress should now show ~0 inserts and ~M updates (because file mtime changed, all existing rows are matched by persistent_id). No duplicates.

## DB verification

```bash
sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "
  SELECT COUNT(*) FROM tracks;
  SELECT COUNT(*) FROM playlists;
  SELECT kind, COUNT(*) FROM playlists GROUP BY kind;
  SELECT name, kind, parent_id FROM playlists LIMIT 10;
"
```

Expected: track count matches `itl_rs::ItlFile::tracks().len()` (~51K for the reference library, minus any with missing persistent_id). Playlist counts: ~422 regular, 0 smart detected by itl-rs on iTunes 12+ libraries (see Known gaps), ~16 folder.

## Known gaps (follow-up phases)

- **Smart playlists import as snapshots only.** iTunes 12+ doesn't store `SmartPlaylistXml` at the documented subtype, so `itl_rs::Playlist::is_smart()` returns `false` for those playlists and they reconcile as `kind='regular'` with the snapshot of track IDs iTunes last computed. Live smart-rule evaluation is deferred (design doc has the format notes; a future phase can add a decoder once the iTunes 12+ format is reverse-engineered).
- **Managed-root file copy.** The wizard accepts the `auto_copy_files` toggle and persists it on `SyncSource`, but the reconciler doesn't act on it yet. Deferred to Phase 4.
- **Import wizard cancellation.** No cancel button during a running sync.
- **Validation warning for unreachable `to` paths.** If a drive isn't mounted, you get N warnings (one per track) rather than a single upfront error.

## Rolling back

To discard a bad import and retry:

```bash
sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "DELETE FROM sync_sources;"
# playlists / tracks with sync_source_id FK will cascade to SET NULL
# (schema has ON DELETE SET NULL) — follow with:
sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "
  DELETE FROM tracks WHERE sync_source_id IS NULL AND persistent_id IS NOT NULL;
  DELETE FROM playlists WHERE sync_source_id IS NULL AND persistent_id IS NOT NULL;
"
```
