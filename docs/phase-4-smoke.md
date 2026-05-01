# Phase 4 Smoke Test

Manual verification that the managed-library features (copy-on-add, copy-on-sync, organize-on-edit, verify, trash) work end-to-end.

## Pre-reqs

- `develop` merged through Phase 4 (or running on `feature/phase-4-file-management`).
- `~/Music/iTunes/iTunes Library.itl` present (for the sync integration check).
- `npm ci && cd src-tauri && cargo build` clean.
- A dev audio file on disk, e.g. `~/Music/test.flac` or similar. A few is better for organize-on-edit testing.

## Steps

### 1. First launch — library root defaults

```bash
cd .worktrees/feature/phase-4-file-management
npx tauri dev
```

- App opens. Click the sidebar's **Preferences…** button.
- Panel opens at the default state:
  - Library root: `~/Music/TuxTunes` (or wherever `$HOME/Music/TuxTunes` resolves).
  - Organize scheme: `{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}`.
  - Keep library organized: checked.
- Confirm the live preview shows `The Beatles/Abbey Road/03 - Something.flac`.
- Leave defaults; close the panel.

### 2. Copy-on-add (single file)

- Click the sidebar's **Add Files…** button.
- Pick a `.flac` or `.mp3` from outside `~/Music/TuxTunes/`.
- Wait a second. Confirm:
  - The file now also exists at `~/Music/TuxTunes/<artist>/<album>/...`.
  - `sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "SELECT file_path, file_hash, artwork_path FROM tracks ORDER BY id DESC LIMIT 1"` returns:
    - `file_path` under `~/Music/TuxTunes/...`.
    - `file_hash` is a non-empty 16-char hex string.
    - `artwork_path` is non-NULL if the source had embedded cover art (check with `ls -la "$(dirname "$new_path")"/cover.*`).

### 3. Sync with `auto_copy_files = true`

- Open the iTunes import wizard (sidebar → **Import iTunes library…**).
- Pick `~/Music/iTunes/iTunes Library.itl`.
- On the map step, set mappings to point at a real mount.
- On the conflict step, verify `auto_copy_files` is implicit-true (the backend default). **Note:** the wizard currently doesn't expose the toggle explicitly; you can flip it via SQL before running sync: `sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "UPDATE sync_sources SET auto_copy_files = 1 WHERE id = ?"`.
- Click **Sync now**. Watch the progress panel cycle through decoding → applying_tracks → applying_playlists → finalizing.
- After completion, the sync emits individual `fs:ingest-*` events per track — DevTools → Network/Events panel should show them streaming.
- After a few minutes, confirm `ls ~/Music/TuxTunes/` starts filling up with artist folders.

### 4. Organize-on-edit (via SQL — no editor UI yet)

Metadata editor UI is deferred to a later phase. For now, trigger the organize worker directly via SQL + the `reorganize_track` command.

1. Pick a track that was ingested in step 2 or 3.
   ```bash
   sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "SELECT id, title, file_path FROM tracks ORDER BY id DESC LIMIT 1"
   ```
2. Change its title in the DB:
   ```bash
   sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "UPDATE tracks SET title = 'New Title' WHERE id = <id>"
   ```
3. Invoke `reorganize_track` via DevTools console:
   ```js
   await window.__TAURI__.core.invoke('reorganize_track', { trackId: <id> });
   ```
4. Confirm:
   - The file moved: `ls -la` the old path (should be gone) and the new path (should exist).
   - Any parent directories that became empty got removed.
   - `sqlite3 ... "SELECT file_path FROM tracks WHERE id = <id>"` returns the new path.

### 5. Verify Library — re-hash + flag mismatches

1. Pick a track and mangle its file content:
   ```bash
   echo "garbage" >> <its file_path>
   ```
2. Invoke `verify_library` via DevTools:
   ```js
   await window.__TAURI__.core.invoke('verify_library');
   ```
3. Listen for `fs:verify-progress` and `fs:verify-complete` events in DevTools.
4. After completion:
   ```bash
   sqlite3 ~/.local/share/tuxtunes/tuxtunes.db "SELECT import_status FROM tracks WHERE id = <id>"
   ```
   Should report `missing_source` for the mangled track. Untouched tracks stay `ok` with a refreshed `verified_at`.

### 6. Trash + remove

- Pick a test track id.
- Via DevTools:
  ```js
  await window.__TAURI__.core.invoke('trash_track', { trackId: <id> });
  ```
- Confirm:
  - File lands in `~/.local/share/Trash/files/`.
  - Row is gone from `tracks` (`sqlite3 ... "SELECT COUNT(*) FROM tracks WHERE id = <id>"` → 0).
- `remove_track` behaves the same minus the trash step:
  ```js
  await window.__TAURI__.core.invoke('remove_track', { trackId: <id> });
  ```

## Known gaps (future work)

- **Metadata editor UI** — organize-on-edit is exercised via SQL + `reorganize_track` invoke; a real edit surface is deferred.
- **Tag write-through on edit** — the organize worker renames files but does not write updated tags back to the audio file. Coupled with the editor UI above.
- **Progress reporting for verify** — emits every 50 tracks; the UI has no surface for it yet.
- **First-run setup prompt** for the managed library root — we auto-create `$HOME/Music/TuxTunes` silently. A permission/disk-space failure just propagates an error today.
- **Bulk "Re-organize library"** — `reorganize_track` is per-track; a bulk action would fan out over every row. The coordinator's worker channel already serializes correctly; just needs a command + UI button.

## Rolling back

To wipe the test DB and start fresh:

```bash
rm ~/.local/share/tuxtunes/tuxtunes.db
rm -rf ~/Music/TuxTunes   # delete the managed files too if desired
```
