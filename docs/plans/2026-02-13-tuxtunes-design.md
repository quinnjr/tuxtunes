# TuxTunes Design Document

## Overview

TuxTunes is a desktop music library manager and player for Linux, designed as an iTunes replacement. It imports an existing iTunes library (XML + media files) from a Windows installation, preserving all metadata, playlists (including smart playlists with full rule parity), and folder hierarchy.

## Tech Stack

| Layer | Technology |
|---|---|
| Desktop shell | Tauri 2.x (system webview) |
| Frontend | Angular 21+ (standalone components, signals, zoneless) |
| Styling | TailwindCSS 4+ |
| Backend | Rust (Tauri commands) |
| Database | SQLite via `rusqlite` |
| Playback | `libmpv` via Rust FFI |
| iTunes import | `plist` crate + custom smart playlist binary decoder |
| Audio metadata | `lofty` crate |

## Architecture

Monolithic single-binary Tauri application. Angular frontend communicates with the Rust backend via Tauri `invoke()` commands. Real-time updates (playback position, import progress) flow from Rust to Angular via Tauri events.

```
Angular Frontend                    Tauri Rust Backend
─────────────────                   ──────────────────
                    invoke()
library.service ──────────────────> commands/library.rs
                                        │
playback.service ─────────────────> commands/playback.rs
                                        │         │
playlist.service ─────────────────> commands/playlist.rs
                                        │         │
                                        ▼         ▼
                    listen()        db/        playback/
  <──────────────────────────       (rusqlite)  (libmpv)
  (events: track-changed,
   position-update, import-progress)
```

## Project Structure

```
tuxtunes/
├── src/                          # Angular frontend
│   ├── app/
│   │   ├── app.component.ts
│   │   ├── app.component.html
│   │   ├── app.config.ts
│   │   ├── app.routes.ts
│   │   ├── components/
│   │   │   ├── sidebar/
│   │   │   ├── track-list/
│   │   │   ├── transport/
│   │   │   ├── now-playing/
│   │   │   ├── import-wizard/
│   │   │   └── smart-playlist-editor/
│   │   ├── services/
│   │   │   ├── tauri.service.ts
│   │   │   ├── library.service.ts
│   │   │   ├── playback.service.ts
│   │   │   └── playlist.service.ts
│   │   └── models/
│   │       ├── track.model.ts
│   │       ├── playlist.model.ts
│   │       └── smart-rule.model.ts
│   ├── styles.css
│   ├── index.html
│   └── main.ts
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   │   └── default.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs
│       ├── db/
│       │   ├── mod.rs
│       │   ├── schema.rs
│       │   ├── tracks.rs
│       │   └── playlists.rs
│       ├── import/
│       │   ├── mod.rs
│       │   ├── itunes_xml.rs
│       │   ├── smart_playlist.rs
│       │   └── path_rewriter.rs
│       ├── playback/
│       │   ├── mod.rs
│       │   └── mpv.rs
│       └── commands/
│           ├── mod.rs
│           ├── library.rs
│           ├── playback.rs
│           ├── playlist.rs
│           └── import.rs
├── angular.json
├── package.json
├── tsconfig.json
└── .postcssrc.json
```

## Database Schema (SQLite)

### tracks

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PRIMARY KEY | Auto-increment |
| persistent_id | TEXT UNIQUE | iTunes Persistent ID |
| title | TEXT | |
| artist | TEXT | |
| album_artist | TEXT | |
| album | TEXT | |
| composer | TEXT | |
| genre | TEXT | |
| grouping | TEXT | |
| comment | TEXT | |
| year | INTEGER | |
| track_number | INTEGER | |
| track_count | INTEGER | |
| disc_number | INTEGER | |
| disc_count | INTEGER | |
| bpm | INTEGER | |
| duration_ms | INTEGER | Total Time in milliseconds |
| size_bytes | INTEGER | |
| bit_rate | INTEGER | |
| sample_rate | INTEGER | |
| kind | TEXT | e.g. "MPEG audio file" |
| file_path | TEXT NOT NULL | Resolved Linux path |
| rating | INTEGER DEFAULT 0 | 0-100 (20 per star) |
| play_count | INTEGER DEFAULT 0 | |
| skip_count | INTEGER DEFAULT 0 | |
| last_played | TEXT | ISO 8601 datetime |
| last_skipped | TEXT | |
| date_added | TEXT NOT NULL | |
| date_modified | TEXT | |
| release_date | TEXT | |
| compilation | BOOLEAN DEFAULT 0 | |
| sort_title | TEXT | |
| sort_artist | TEXT | |
| sort_album | TEXT | |
| sort_album_artist | TEXT | |
| sort_composer | TEXT | |
| artwork_path | TEXT | Path to extracted album art |
| protected | BOOLEAN DEFAULT 0 | DRM flag |
| purchased | BOOLEAN DEFAULT 0 | |
| itunes_track_id | INTEGER | Original iTunes Track ID |

Indexes: artist, album, genre, album_artist, rating, play_count, date_added, file_path.

### playlists

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PRIMARY KEY | |
| name | TEXT NOT NULL | |
| persistent_id | TEXT UNIQUE | iTunes Persistent ID |
| is_smart | BOOLEAN DEFAULT 0 | |
| is_folder | BOOLEAN DEFAULT 0 | |
| parent_id | INTEGER REFERENCES playlists(id) | Folder hierarchy |
| sort_order | INTEGER | Display ordering |

### playlist_tracks

| Column | Type | Notes |
|---|---|---|
| playlist_id | INTEGER | FK -> playlists(id) ON DELETE CASCADE |
| track_id | INTEGER | FK -> tracks(id) ON DELETE CASCADE |
| position | INTEGER NOT NULL | Ordering within playlist |

PRIMARY KEY (playlist_id, track_id).

### smart_playlist_rules

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PRIMARY KEY | |
| playlist_id | INTEGER | FK -> playlists(id) ON DELETE CASCADE |
| match_all | BOOLEAN DEFAULT 1 | AND vs OR |
| limit_enabled | BOOLEAN DEFAULT 0 | |
| limit_value | INTEGER | |
| limit_type | TEXT | 'songs', 'minutes', 'hours', 'mb', 'gb' |
| limit_sort | TEXT | 'random', 'artist', 'most_played', etc. |
| live_updating | BOOLEAN DEFAULT 1 | |

### smart_playlist_conditions

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PRIMARY KEY | |
| rule_id | INTEGER | FK -> smart_playlist_rules(id) ON DELETE CASCADE |
| parent_group_id | INTEGER | FK -> self, for nested groups |
| is_group | BOOLEAN DEFAULT 0 | |
| group_match_all | BOOLEAN DEFAULT 1 | AND/OR for nested groups |
| field | TEXT | 'artist', 'album', 'genre', etc. |
| operator | TEXT | 'is', 'contains', 'greater_than', etc. |
| value_text | TEXT | |
| value_int | INTEGER | |
| value_date | TEXT | |
| value_int2 | INTEGER | Range end |
| value_date2 | TEXT | Date range end |
| value_units | TEXT | 'days', 'weeks', 'months' |
| position | INTEGER | Ordering |

### preferences

| Column | Type | Notes |
|---|---|---|
| key | TEXT PRIMARY KEY | |
| value | TEXT NOT NULL | |

## iTunes Import Pipeline

### Flow

1. User triggers File > Import iTunes Library
2. File picker for `iTunes Music Library.xml`
3. Path mapping dialog: detect drive letter prefixes (D:, C:), user maps each to Linux mount point OR chooses "copy to local library" OR "skip"
4. Streaming parse via `plist` crate:
   - Parse tracks dict, insert into SQLite in batches of 1000 (transaction per batch)
   - Rewrite file paths: `file://localhost/D:/...` -> mapped Linux path
   - URL-decode percent-encoded characters
   - If copy mode: queue file copies to background thread
5. Parse playlists:
   - Import folders first (resolve hierarchy by Persistent ID)
   - Import regular playlists (map iTunes Track IDs to local DB IDs)
   - Import smart playlists: decode Smart Info + Smart Criteria binary blobs
6. Progress updates via Tauri events (tracks imported, playlists done, files copied, errors)
7. Optional verify pass: check file paths exist on disk, flag missing files

### Path Rewriting

| iTunes Path | Linux Path |
|---|---|
| `file://localhost/D:/Users/Joseph/Music/...` | `/run/media/joseph/Local Disk/Users/Joseph/Music/...` |
| `%20` in URLs | Decoded to spaces |
| `&#38;` XML entity | `&` |
| Double slashes `//Music/` | Normalized to single `/Music/` |

### Source Library Stats (reference)

- 89MB XML file, 2.1M lines
- 42,146 tracks with file locations (42,025 on D:, 121 on C:)
- 347,267 total Track ID entries (includes duplicates across playlists)
- File types: MP3 (20,544), Purchased AAC (16,292), AAC (3,632), Protected AAC (720), ALAC (595), WAV (371), video (123), Matched AAC (24), AIFF (12)
- 434 playlists total: 15 regular, 408 user-created smart playlists, 9 genre folders, system playlists
- Folder hierarchy: 9 genre folders (Alternative, Electronic/Techno, Indie, Jazz, Metal, Other, New Age, Rock, Soundtrack) with all 408 smart playlists nested inside

## Smart Playlist Rule Engine

### Binary Format Decoder

Decodes iTunes Smart Info and Smart Criteria binary blobs:
- Smart Info: limit settings (count/size/time), sort order, live-update flag
- Smart Criteria: starts with `SLst` magic bytes, big-endian. Contains field codes, operator codes (bitmapped), and values (string/int/date)

### Field Codes

| Field | Code | Data Type |
|---|---|---|
| Song Name | 0x02 | String |
| Album | 0x03 | String |
| Artist | 0x04 | String |
| Bitrate | 0x05 | Integer |
| Sample Rate | 0x06 | Integer |
| Year | 0x07 | Integer |
| Genre | 0x08 | String |
| Kind | 0x09 | String |
| Date Modified | 0x0a | Date |
| Track Number | 0x0b | Integer |
| Size | 0x0c | Integer |
| Time | 0x0d | Integer |
| Comment | 0x0e | String |
| Date Added | 0x10 | Date |
| Composer | 0x12 | String |
| Play Count | 0x16 | Integer |
| Last Played | 0x17 | Date |
| Disc Number | 0x18 | Integer |
| Rating | 0x19 | Integer |
| Compilation | 0x1f | Boolean |
| BPM | 0x23 | Integer |
| Grouping | 0x27 | String |
| Playlist | 0x28 | Playlist ref |
| Skip Count | 0x44 | Integer |
| Last Skipped | 0x45 | Date |
| Album Artist | 0x47 | String |

### SQL Generation

Smart playlists are evaluated by generating SQL WHERE clauses from the condition tree.

Example: "Recently Played Rock" with conditions: genre IS "Rock" AND last_played IN THE LAST 30 days AND rating > 60:
```sql
SELECT t.* FROM tracks t
WHERE t.genre = 'Rock'
  AND t.last_played >= datetime('now', '-30 days')
  AND t.rating > 60
```

Nested groups produce nested parenthesized clauses with AND/OR.

### Operators (Full iTunes Parity)

| Operator | String | Numeric | Date |
|---|---|---|---|
| is / is not | Exact match | Equality | Exact date |
| contains / does not contain | LIKE %val% | -- | -- |
| starts with / does not start with | LIKE val% | -- | -- |
| ends with / does not end with | LIKE %val | -- | -- |
| greater than / less than | -- | Comparison | Before/after |
| in the range | -- | BETWEEN | Date range |
| in the last / not in the last | -- | -- | datetime('now', '-N units') |

### Limit Options

- Limit to N songs / N minutes / N hours / N MB / N GB
- Selected by: random, song name, album, artist, genre, most recently added, most often played, most recently played, highest rating

### Live Updating

When enabled, smart playlists re-evaluate when:
- Track play count or last played changes
- Track rating changes
- Track added or removed from library

Cached result sets invalidated on relevant changes.

### Playlist-references-Playlist

Smart playlists can reference other playlists:
```sql
WHERE t.id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?)
```

## Playback Engine (libmpv)

- Embedded libmpv as in-process library via Rust FFI
- Handles all codec decoding: MP3, AAC, M4A, FLAC, ALAC, WAV, AIFF, Opus
- Protected M4P files detected and flagged (cannot play DRM content)

### Features

- Play/Pause/Stop/Next/Previous transport controls
- Seek with position tracking
- Volume control via mpv `volume` property
- Internal play queue
- Gapless playback (native mpv support)
- Play count tracking: increment after 50% or 30 seconds (whichever is longer)
- Skip count tracking: increment when skipped before play-count threshold

### Event Bridge

mpv events (end-of-file, property changes) bridged to Angular via Tauri events:
- `track-changed`: emitted when current track changes
- `position-update`: emitted periodically with current position/duration
- `playback-state-changed`: play/pause/stop state changes

## UI Design (Angular + TailwindCSS 4)

### Theme

Dark theme, music-player aesthetic:
```css
@import "tailwindcss";

@theme {
  --color-bg-primary: #1a1a2e;
  --color-bg-secondary: #16213e;
  --color-bg-tertiary: #0f3460;
  --color-accent: #e94560;
  --color-accent-hover: #ff6b81;
  --color-text-primary: #eaeaea;
  --color-text-secondary: #a0a0b0;
  --color-border: #2a2a4a;
}
```

### Layout

```
┌─────────────────────────────────────────────────────────────┐
│  Transport Bar                                              │
│  [<<] [Play/Pause] [>>]  ───●─────── 2:30/4:15   Vol ═══  │
│  Now Playing: Rise Against - The Good Left Undone           │
├──────────────┬──────────────────────────────────────────────┤
│  Sidebar     │  Track List (virtual scroll, CDK)            │
│  Library     │  Search...                     [Columns]     │
│   All Songs  │  # | Title | Artist | Album | Time | Rating  │
│   Artists    │  ...                                         │
│   Albums     │                                              │
│   Genres     │                                              │
│  ──────────  │                                              │
│  Playlists   │                                              │
│  ▸ Alt.      │                                              │
│  ▸ Metal     │                                              │
│  ▸ Rock      │                                              │
│  ...         │                                              │
├──────────────┴──────────────────────────────────────────────┤
│  Status: 42,313 songs | 125.3 days | 198.5 GB              │
└─────────────────────────────────────────────────────────────┘
```

### Virtual Scrolling

347K tracks require virtual scrolling:
- Angular CDK `cdk-virtual-scroll-viewport` renders ~50 visible rows
- On scroll, `invoke('get_tracks', { offset, limit: 100, sort, filter })` pages from SQLite
- Total count from `invoke('get_track_count', { filter })` sets scroll container height

### Key Components

- **transport**: Play/pause/skip buttons, seek/volume sliders, now-playing info. Subscribes to Tauri `position-update` events.
- **sidebar**: Collapsible tree with playlist folders. Recursive Angular `@for` template.
- **track-list**: Virtual-scrolled table with sortable columns. Paginated from Rust.
- **import-wizard**: Multi-step modal (file pick, path mapping, progress). Uses Tauri event streaming for progress.
- **smart-playlist-editor**: Rule builder with add/remove conditions, field/operator/value pickers, nested group support.

## Preferences (stored in SQLite)

Key-value pairs in `preferences` table:
- `volume`: 0-100
- `last_playlist_id`: Last viewed playlist
- `last_track_id`: Last played track
- `last_position_ms`: Seek position of last track
- `sidebar_width`: Pixel width
- `sort_column`: Current sort column
- `sort_direction`: asc/desc
- `import_path_mappings`: JSON blob of drive letter -> Linux path mappings
- `music_library_path`: Local library root (for copy-mode imports)
- `theme`: dark/light (future)
