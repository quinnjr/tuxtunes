# Phase 2 Smoke Test

Manual verification that the playback engine plays a file end-to-end.

## Setup

1. `npm ci`
2. `npm run codegen`
3. `cd src-tauri && cargo build`
4. `cd .. && npx tauri dev`

## Steps

1. App window opens. Transport bar at top says "Nothing playing".
2. Click **Add Files…** in the sidebar. Pick a FLAC, MP3, or WAV.
3. Track appears in the main list.
4. Double-click the track. Transport bar updates with title/artist/album;
   play icon flips to pause.
5. Click pause. Icon flips back. Click again: resumes.
6. Drag seek slider. Position jumps; playback continues.
7. Drag volume slider. Volume audibly changes.
8. Let track play past 50% (or 30s). Close the app cleanly. Reopen.
   Track's `play_count` in the `All Songs` list increments by one
   (verify by inspecting with `sqlite3 ~/.local/share/tuxtunes/tuxtunes.db
"SELECT title, play_count, skip_count FROM tracks"`).
9. Switch to **Settings** tab. Pick a different device. Pick the track
   again. Playback routes to the new device.

## Known gaps (Phase 3+)

- Queue of multiple tracks (Phase 6).
- Sample-rate/bit-depth chip visible in the transport bar (Phase 6 polish).
- DSD file confirmation on DSD-capable hardware (best-effort on Phase 2).
