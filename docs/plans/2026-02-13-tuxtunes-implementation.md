# TuxTunes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a desktop music library manager that imports iTunes libraries (including smart playlists) and plays music via libmpv, using Tauri 2 + Angular 21 + TailwindCSS 4.

**Architecture:** Monolithic Tauri 2 app. Rust backend handles SQLite database, libmpv playback, and iTunes import. Angular frontend communicates via Tauri `invoke()` commands and `listen()` events. Virtual scrolling for 347K+ track libraries.

**Tech Stack:** Tauri 2.x, Angular 21+ (standalone/signals/zoneless), TailwindCSS 4, Rust, rusqlite, libmpv, plist crate, lofty crate

**Reference:** `docs/plans/2026-02-13-tuxtunes-design.md`

---

## Phase 1: Project Scaffolding

### Task 1: Scaffold Tauri + Angular Project

**Files:**
- Create: project root via `create-tauri-app`
- Modify: `src-tauri/Cargo.toml` (add dependencies)
- Modify: `package.json` (add Angular CDK)
- Modify: `src-tauri/tauri.conf.json` (app config)

**Step 1: Create Tauri + Angular scaffold**

The existing `Cargo.toml` and `src/main.rs` at the repo root are placeholders from the initial commit. The Tauri scaffold will create a proper project structure with `src-tauri/` for Rust and `src/` for Angular. Remove the old placeholder files first.

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes
rm -f Cargo.toml src/main.rs && rmdir src
npm create tauri-app@latest . -- --template angular --manager npm
```

If `create-tauri-app` does not support `--template angular` directly, scaffold manually:

```bash
# Create Angular project in the current directory
ng new tuxtunes --directory . --routing --style css --skip-git
# Then add Tauri
npm install @tauri-apps/cli@latest
npx tauri init
```

When prompted by `tauri init`:
- App name: `TuxTunes`
- Window title: `TuxTunes`
- Frontend dev URL: `http://localhost:4200`
- Frontend dev command: `npm run start`
- Frontend build command: `npm run build`
- Frontend dist dir: `../dist/tuxtunes/browser`

**Step 2: Add Rust dependencies**

Edit `src-tauri/Cargo.toml` dependencies section:

```toml
[package]
name = "tuxtunes"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-dialog = "2"
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
plist = "1"
lofty = "0.22"
base64 = "0.22"
percent-encoding = "2"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"
log = "0.4"
env_logger = "0.11"
byteorder = "1"

[build-dependencies]
tauri-build = { version = "2", features = [] }
```

Note: We are NOT adding libmpv as a crate dependency yet. We will integrate mpv via `mpv-client` or direct FFI in Task 12. We will determine the best available crate at that point and add it.

**Step 3: Add Angular dependencies**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes
npm install @tauri-apps/api@^2
npm install @angular/cdk@^21
```

**Step 4: Verify build compiles**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes
npm run build
cd src-tauri && cargo check
```

Expected: Both compile without errors.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: scaffold Tauri 2 + Angular 21 project"
```

---

### Task 2: Configure TailwindCSS 4 + Dark Theme

**Files:**
- Modify: `package.json` (tailwind deps)
- Create: `.postcssrc.json`
- Modify: `src/styles.css` (theme)
- Modify: `src/index.html` (dark bg fallback)

**Step 1: Install TailwindCSS 4**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes
npm install -D tailwindcss @tailwindcss/postcss postcss
```

**Step 2: Create PostCSS config**

Create `.postcssrc.json`:
```json
{
  "plugins": {
    "@tailwindcss/postcss": {}
  }
}
```

**Step 3: Configure global styles with dark theme**

Replace `src/styles.css`:
```css
@import "tailwindcss";

@theme {
  --color-bg-primary: #1a1a2e;
  --color-bg-secondary: #16213e;
  --color-bg-tertiary: #0f3460;
  --color-bg-hover: #1e2a4a;
  --color-accent: #e94560;
  --color-accent-hover: #ff6b81;
  --color-text-primary: #eaeaea;
  --color-text-secondary: #a0a0b0;
  --color-text-muted: #6a6a8a;
  --color-border: #2a2a4a;
  --color-scrollbar: #3a3a5a;
  --color-scrollbar-hover: #4a4a6a;
  --color-success: #4ade80;
  --color-warning: #fbbf24;
  --color-error: #f87171;
}

body {
  @apply bg-bg-primary text-text-primary;
  font-family: system-ui, -apple-system, sans-serif;
  overflow: hidden;
  user-select: none;
}

/* Custom scrollbar styling */
::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}

::-webkit-scrollbar-track {
  background: transparent;
}

::-webkit-scrollbar-thumb {
  background: var(--color-scrollbar);
  border-radius: 4px;
}

::-webkit-scrollbar-thumb:hover {
  background: var(--color-scrollbar-hover);
}
```

**Step 4: Update index.html with dark background**

Add to `src/index.html` `<body>` tag:
```html
<body class="bg-bg-primary text-text-primary">
```

**Step 5: Verify Tailwind works**

Update `src/app/app.component.html` temporarily:
```html
<div class="flex items-center justify-center h-screen">
  <h1 class="text-4xl font-bold text-accent">TuxTunes</h1>
</div>
```

Run: `npm run start`
Expected: Page shows "TuxTunes" in the accent red color (#e94560) centered on dark background.

**Step 6: Commit**

```bash
git add -A
git commit -m "feat: configure TailwindCSS 4 with dark music player theme"
```

---

## Phase 2: Database Foundation

### Task 3: SQLite Schema + Database Manager

**Files:**
- Create: `src-tauri/src/db/mod.rs`
- Create: `src-tauri/src/db/schema.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/main.rs`

**Step 1: Create db module structure**

Create `src-tauri/src/db/mod.rs`:
```rust
pub mod schema;
pub mod tracks;
pub mod playlists;

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;

pub struct Database {
    pub conn: Mutex<Connection>,
}

impl Database {
    pub fn new(app_handle: &AppHandle) -> Result<Self, rusqlite::Error> {
        let app_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        std::fs::create_dir_all(&app_dir).expect("failed to create app data dir");

        let db_path = app_dir.join("tuxtunes.db");
        let conn = Connection::open(&db_path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        schema::run_migrations(&conn)?;

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory database for testing
    #[cfg(test)]
    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        schema::run_migrations(&conn)?;
        Ok(Database {
            conn: Mutex::new(conn),
        })
    }
}
```

**Step 2: Create schema with migrations**

Create `src-tauri/src/db/schema.rs`:
```rust
use rusqlite::Connection;

pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );

        INSERT OR IGNORE INTO schema_version (rowid, version) VALUES (1, 0);"
    )?;

    let version: i64 = conn.query_row(
        "SELECT version FROM schema_version WHERE rowid = 1",
        [],
        |row| row.get(0),
    )?;

    if version < 1 {
        migrate_v1(conn)?;
    }

    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE tracks (
            id INTEGER PRIMARY KEY,
            persistent_id TEXT UNIQUE,
            title TEXT,
            artist TEXT,
            album_artist TEXT,
            album TEXT,
            composer TEXT,
            genre TEXT,
            grouping TEXT,
            comment TEXT,
            year INTEGER,
            track_number INTEGER,
            track_count INTEGER,
            disc_number INTEGER,
            disc_count INTEGER,
            bpm INTEGER,
            duration_ms INTEGER,
            size_bytes INTEGER,
            bit_rate INTEGER,
            sample_rate INTEGER,
            kind TEXT,
            file_path TEXT NOT NULL,
            rating INTEGER NOT NULL DEFAULT 0,
            play_count INTEGER NOT NULL DEFAULT 0,
            skip_count INTEGER NOT NULL DEFAULT 0,
            last_played TEXT,
            last_skipped TEXT,
            date_added TEXT NOT NULL,
            date_modified TEXT,
            release_date TEXT,
            compilation INTEGER NOT NULL DEFAULT 0,
            sort_title TEXT,
            sort_artist TEXT,
            sort_album TEXT,
            sort_album_artist TEXT,
            sort_composer TEXT,
            artwork_path TEXT,
            protected INTEGER NOT NULL DEFAULT 0,
            purchased INTEGER NOT NULL DEFAULT 0,
            itunes_track_id INTEGER
        );

        CREATE INDEX idx_tracks_artist ON tracks(artist);
        CREATE INDEX idx_tracks_album ON tracks(album);
        CREATE INDEX idx_tracks_genre ON tracks(genre);
        CREATE INDEX idx_tracks_album_artist ON tracks(album_artist);
        CREATE INDEX idx_tracks_rating ON tracks(rating);
        CREATE INDEX idx_tracks_play_count ON tracks(play_count);
        CREATE INDEX idx_tracks_date_added ON tracks(date_added);
        CREATE INDEX idx_tracks_file_path ON tracks(file_path);

        CREATE TABLE playlists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            persistent_id TEXT UNIQUE,
            is_smart INTEGER NOT NULL DEFAULT 0,
            is_folder INTEGER NOT NULL DEFAULT 0,
            parent_id INTEGER REFERENCES playlists(id) ON DELETE SET NULL,
            sort_order INTEGER
        );

        CREATE INDEX idx_playlists_parent ON playlists(parent_id);

        CREATE TABLE playlist_tracks (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            position INTEGER NOT NULL,
            PRIMARY KEY (playlist_id, track_id)
        );

        CREATE TABLE smart_playlist_rules (
            id INTEGER PRIMARY KEY,
            playlist_id INTEGER NOT NULL UNIQUE REFERENCES playlists(id) ON DELETE CASCADE,
            match_all INTEGER NOT NULL DEFAULT 1,
            limit_enabled INTEGER NOT NULL DEFAULT 0,
            limit_value INTEGER,
            limit_type TEXT,
            limit_sort TEXT,
            live_updating INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE smart_playlist_conditions (
            id INTEGER PRIMARY KEY,
            rule_id INTEGER NOT NULL REFERENCES smart_playlist_rules(id) ON DELETE CASCADE,
            parent_group_id INTEGER REFERENCES smart_playlist_conditions(id) ON DELETE CASCADE,
            is_group INTEGER NOT NULL DEFAULT 0,
            group_match_all INTEGER NOT NULL DEFAULT 1,
            field TEXT,
            operator TEXT,
            value_text TEXT,
            value_int INTEGER,
            value_date TEXT,
            value_int2 INTEGER,
            value_date2 TEXT,
            value_units TEXT,
            position INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX idx_conditions_rule ON smart_playlist_conditions(rule_id);

        CREATE TABLE preferences (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        UPDATE schema_version SET version = 1 WHERE rowid = 1;"
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_cleanly() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        // Verify tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tracks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // Should not error
    }
}
```

**Step 3: Wire up db module in lib.rs**

Update `src-tauri/src/lib.rs`:
```rust
mod db;

use db::Database;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let database = Database::new(app.handle())?;
            app.manage(database);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**Step 4: Create placeholder files for tracks and playlists modules**

Create `src-tauri/src/db/tracks.rs`:
```rust
// Track CRUD operations - implemented in Task 4
```

Create `src-tauri/src/db/playlists.rs`:
```rust
// Playlist CRUD operations - implemented in Task 5
```

**Step 5: Run tests**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes/src-tauri
cargo test
```

Expected: 2 tests pass (migrations run cleanly + idempotent).

**Step 6: Commit**

```bash
git add -A
git commit -m "feat: add SQLite database layer with schema migrations"
```

---

### Task 4: Track CRUD Operations

**Files:**
- Modify: `src-tauri/src/db/tracks.rs`

**Step 1: Write failing tests for track operations**

Replace `src-tauri/src/db/tracks.rs`:
```rust
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub persistent_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub composer: Option<String>,
    pub genre: Option<String>,
    pub grouping: Option<String>,
    pub comment: Option<String>,
    pub year: Option<i64>,
    pub track_number: Option<i64>,
    pub track_count: Option<i64>,
    pub disc_number: Option<i64>,
    pub disc_count: Option<i64>,
    pub bpm: Option<i64>,
    pub duration_ms: Option<i64>,
    pub size_bytes: Option<i64>,
    pub bit_rate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub kind: Option<String>,
    pub file_path: String,
    pub rating: i64,
    pub play_count: i64,
    pub skip_count: i64,
    pub last_played: Option<String>,
    pub last_skipped: Option<String>,
    pub date_added: String,
    pub date_modified: Option<String>,
    pub release_date: Option<String>,
    pub compilation: bool,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    pub sort_composer: Option<String>,
    pub artwork_path: Option<String>,
    pub protected: bool,
    pub purchased: bool,
    pub itunes_track_id: Option<i64>,
}

/// Lightweight track struct for list views (avoids sending all fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackSummary {
    pub id: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub duration_ms: Option<i64>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub year: Option<i64>,
    pub rating: i64,
    pub play_count: i64,
    pub date_added: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackQuery {
    pub offset: i64,
    pub limit: i64,
    pub sort_column: Option<String>,
    pub sort_direction: Option<String>, // "asc" or "desc"
    pub search: Option<String>,
    pub genre: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playlist_id: Option<i64>,
}

/// Insert a new track, returning its ID.
pub fn insert_track(conn: &Connection, track: &Track) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO tracks (
            persistent_id, title, artist, album_artist, album, composer, genre,
            grouping, comment, year, track_number, track_count, disc_number,
            disc_count, bpm, duration_ms, size_bytes, bit_rate, sample_rate,
            kind, file_path, rating, play_count, skip_count, last_played,
            last_skipped, date_added, date_modified, release_date, compilation,
            sort_title, sort_artist, sort_album, sort_album_artist, sort_composer,
            artwork_path, protected, purchased, itunes_track_id
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
            ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39
        )",
        params![
            track.persistent_id, track.title, track.artist, track.album_artist,
            track.album, track.composer, track.genre, track.grouping, track.comment,
            track.year, track.track_number, track.track_count, track.disc_number,
            track.disc_count, track.bpm, track.duration_ms, track.size_bytes,
            track.bit_rate, track.sample_rate, track.kind, track.file_path,
            track.rating, track.play_count, track.skip_count, track.last_played,
            track.last_skipped, track.date_added, track.date_modified,
            track.release_date, track.compilation, track.sort_title,
            track.sort_artist, track.sort_album, track.sort_album_artist,
            track.sort_composer, track.artwork_path, track.protected,
            track.purchased, track.itunes_track_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert tracks in a batch within a single transaction.
pub fn insert_tracks_batch(conn: &Connection, tracks: &[Track]) -> Result<usize, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    let mut count = 0;
    for track in tracks {
        insert_track(&tx, track)?;
        count += 1;
    }
    tx.commit()?;
    Ok(count)
}

const SUMMARY_COLUMNS: &str =
    "id, title, artist, album, album_artist, genre, duration_ms, track_number, disc_number, year, rating, play_count, date_added, file_path";

fn row_to_summary(row: &rusqlite::Row) -> Result<TrackSummary, rusqlite::Error> {
    Ok(TrackSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        album_artist: row.get(4)?,
        genre: row.get(5)?,
        duration_ms: row.get(6)?,
        track_number: row.get(7)?,
        disc_number: row.get(8)?,
        year: row.get(9)?,
        rating: row.get(10)?,
        play_count: row.get(11)?,
        date_added: row.get(12)?,
        file_path: row.get(13)?,
    })
}

/// Allowed sort columns (prevents SQL injection)
fn validate_sort_column(col: &str) -> &str {
    match col {
        "title" | "artist" | "album" | "album_artist" | "genre" | "year"
        | "track_number" | "disc_number" | "duration_ms" | "rating"
        | "play_count" | "date_added" | "bpm" | "skip_count" => col,
        _ => "title",
    }
}

/// Query tracks with pagination, sorting, and filtering.
pub fn query_tracks(conn: &Connection, query: &TrackQuery) -> Result<Vec<TrackSummary>, rusqlite::Error> {
    let mut sql = format!("SELECT {} FROM tracks WHERE 1=1", SUMMARY_COLUMNS);
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref search) = query.search {
        sql.push_str(" AND (title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1 OR album_artist LIKE ?1)");
        param_values.push(Box::new(format!("%{}%", search)));
    }

    if let Some(ref genre) = query.genre {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND genre = ?{}", idx));
        param_values.push(Box::new(genre.clone()));
    }

    if let Some(ref artist) = query.artist {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND (artist = ?{} OR album_artist = ?{})", idx, idx));
        param_values.push(Box::new(artist.clone()));
    }

    if let Some(ref album) = query.album {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND album = ?{}", idx));
        param_values.push(Box::new(album.clone()));
    }

    if let Some(playlist_id) = query.playlist_id {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(
            " AND id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?{})",
            idx
        ));
        param_values.push(Box::new(playlist_id));
    }

    let sort_col = query
        .sort_column
        .as_deref()
        .map(validate_sort_column)
        .unwrap_or("title");
    let sort_dir = match query.sort_direction.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    sql.push_str(&format!(" ORDER BY {} {} LIMIT ? OFFSET ?", sort_col, sort_dir));

    param_values.push(Box::new(query.limit));
    param_values.push(Box::new(query.offset));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_ref.as_slice(), row_to_summary)?;
    rows.collect()
}

/// Count tracks matching a filter.
pub fn count_tracks(conn: &Connection, query: &TrackQuery) -> Result<i64, rusqlite::Error> {
    let mut sql = "SELECT COUNT(*) FROM tracks WHERE 1=1".to_string();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref search) = query.search {
        sql.push_str(" AND (title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1 OR album_artist LIKE ?1)");
        param_values.push(Box::new(format!("%{}%", search)));
    }

    if let Some(ref genre) = query.genre {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND genre = ?{}", idx));
        param_values.push(Box::new(genre.clone()));
    }

    if let Some(ref artist) = query.artist {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND (artist = ?{} OR album_artist = ?{})", idx, idx));
        param_values.push(Box::new(artist.clone()));
    }

    if let Some(ref album) = query.album {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(" AND album = ?{}", idx));
        param_values.push(Box::new(album.clone()));
    }

    if let Some(playlist_id) = query.playlist_id {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(
            " AND id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?{})",
            idx
        ));
        param_values.push(Box::new(playlist_id));
    }

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    conn.query_row(&sql, params_ref.as_slice(), |row| row.get(0))
}

/// Get a single track by ID (full details).
pub fn get_track(conn: &Connection, id: i64) -> Result<Option<Track>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, persistent_id, title, artist, album_artist, album, composer,
                genre, grouping, comment, year, track_number, track_count,
                disc_number, disc_count, bpm, duration_ms, size_bytes, bit_rate,
                sample_rate, kind, file_path, rating, play_count, skip_count,
                last_played, last_skipped, date_added, date_modified, release_date,
                compilation, sort_title, sort_artist, sort_album, sort_album_artist,
                sort_composer, artwork_path, protected, purchased, itunes_track_id
         FROM tracks WHERE id = ?1"
    )?;

    let mut rows = stmt.query_map(params![id], |row| {
        Ok(Track {
            id: row.get(0)?,
            persistent_id: row.get(1)?,
            title: row.get(2)?,
            artist: row.get(3)?,
            album_artist: row.get(4)?,
            album: row.get(5)?,
            composer: row.get(6)?,
            genre: row.get(7)?,
            grouping: row.get(8)?,
            comment: row.get(9)?,
            year: row.get(10)?,
            track_number: row.get(11)?,
            track_count: row.get(12)?,
            disc_number: row.get(13)?,
            disc_count: row.get(14)?,
            bpm: row.get(15)?,
            duration_ms: row.get(16)?,
            size_bytes: row.get(17)?,
            bit_rate: row.get(18)?,
            sample_rate: row.get(19)?,
            kind: row.get(20)?,
            file_path: row.get(21)?,
            rating: row.get(22)?,
            play_count: row.get(23)?,
            skip_count: row.get(24)?,
            last_played: row.get(25)?,
            last_skipped: row.get(26)?,
            date_added: row.get(27)?,
            date_modified: row.get(28)?,
            release_date: row.get(29)?,
            compilation: row.get(30)?,
            sort_title: row.get(31)?,
            sort_artist: row.get(32)?,
            sort_album: row.get(33)?,
            sort_album_artist: row.get(34)?,
            sort_composer: row.get(35)?,
            artwork_path: row.get(36)?,
            protected: row.get(37)?,
            purchased: row.get(38)?,
            itunes_track_id: row.get(39)?,
        })
    })?;

    match rows.next() {
        Some(Ok(track)) => Ok(Some(track)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Update play count and last played timestamp.
pub fn increment_play_count(conn: &Connection, track_id: i64, timestamp: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE tracks SET play_count = play_count + 1, last_played = ?1 WHERE id = ?2",
        params![timestamp, track_id],
    )?;
    Ok(())
}

/// Update skip count and last skipped timestamp.
pub fn increment_skip_count(conn: &Connection, track_id: i64, timestamp: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE tracks SET skip_count = skip_count + 1, last_skipped = ?1 WHERE id = ?2",
        params![timestamp, track_id],
    )?;
    Ok(())
}

/// Update track rating.
pub fn update_rating(conn: &Connection, track_id: i64, rating: i64) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE tracks SET rating = ?1 WHERE id = ?2",
        params![rating, track_id],
    )?;
    Ok(())
}

/// Get library statistics.
#[derive(Debug, Serialize)]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub total_duration_ms: i64,
    pub total_size_bytes: i64,
}

pub fn get_library_stats(conn: &Connection) -> Result<LibraryStats, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0), COALESCE(SUM(size_bytes), 0) FROM tracks",
        [],
        |row| {
            Ok(LibraryStats {
                total_tracks: row.get(0)?,
                total_duration_ms: row.get(1)?,
                total_size_bytes: row.get(2)?,
            })
        },
    )
}

/// Get distinct values for a column (for sidebar navigation).
pub fn get_distinct_values(conn: &Connection, column: &str) -> Result<Vec<String>, rusqlite::Error> {
    let col = validate_sort_column(column);
    let sql = format!(
        "SELECT DISTINCT {} FROM tracks WHERE {} IS NOT NULL ORDER BY {} ASC",
        col, col, col
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    fn make_test_track(title: &str, artist: &str, album: &str) -> Track {
        Track {
            id: 0,
            persistent_id: None,
            title: Some(title.to_string()),
            artist: Some(artist.to_string()),
            album_artist: Some(artist.to_string()),
            album: Some(album.to_string()),
            composer: None,
            genre: Some("Rock".to_string()),
            grouping: None,
            comment: None,
            year: Some(2020),
            track_number: Some(1),
            track_count: None,
            disc_number: Some(1),
            disc_count: None,
            bpm: None,
            duration_ms: Some(240000),
            size_bytes: Some(5000000),
            bit_rate: Some(320),
            sample_rate: Some(44100),
            kind: Some("MPEG audio file".to_string()),
            file_path: format!("/music/{}/{}.mp3", artist, title),
            rating: 80,
            play_count: 5,
            skip_count: 0,
            last_played: None,
            last_skipped: None,
            date_added: "2024-01-01T00:00:00Z".to_string(),
            date_modified: None,
            release_date: None,
            compilation: false,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
            sort_composer: None,
            artwork_path: None,
            protected: false,
            purchased: false,
            itunes_track_id: None,
        }
    }

    #[test]
    fn test_insert_and_get_track() {
        let conn = setup_db();
        let track = make_test_track("The Good Left Undone", "Rise Against", "Sufferer");
        let id = insert_track(&conn, &track).unwrap();
        assert!(id > 0);

        let fetched = get_track(&conn, id).unwrap().unwrap();
        assert_eq!(fetched.title.as_deref(), Some("The Good Left Undone"));
        assert_eq!(fetched.artist.as_deref(), Some("Rise Against"));
        assert_eq!(fetched.rating, 80);
    }

    #[test]
    fn test_batch_insert() {
        let conn = setup_db();
        let tracks = vec![
            make_test_track("Song A", "Artist 1", "Album 1"),
            make_test_track("Song B", "Artist 2", "Album 2"),
            make_test_track("Song C", "Artist 1", "Album 1"),
        ];
        let count = insert_tracks_batch(&conn, &tracks).unwrap();
        assert_eq!(count, 3);

        let stats = get_library_stats(&conn).unwrap();
        assert_eq!(stats.total_tracks, 3);
    }

    #[test]
    fn test_query_tracks_with_search() {
        let conn = setup_db();
        let tracks = vec![
            make_test_track("Prayer Position", "Rise Against", "Sufferer"),
            make_test_track("Survive", "Rise Against", "Sufferer"),
            make_test_track("Enter Sandman", "Metallica", "Black Album"),
        ];
        insert_tracks_batch(&conn, &tracks).unwrap();

        let query = TrackQuery {
            offset: 0,
            limit: 100,
            sort_column: None,
            sort_direction: None,
            search: Some("Rise".to_string()),
            genre: None,
            artist: None,
            album: None,
            playlist_id: None,
        };

        let results = query_tracks(&conn, &query).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_query_tracks_pagination() {
        let conn = setup_db();
        for i in 0..20 {
            let track = make_test_track(&format!("Song {}", i), "Artist", "Album");
            insert_track(&conn, &track).unwrap();
        }

        let query = TrackQuery {
            offset: 5,
            limit: 10,
            sort_column: Some("title".to_string()),
            sort_direction: Some("asc".to_string()),
            search: None,
            genre: None,
            artist: None,
            album: None,
            playlist_id: None,
        };

        let results = query_tracks(&conn, &query).unwrap();
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_increment_play_count() {
        let conn = setup_db();
        let track = make_test_track("Test", "Artist", "Album");
        let id = insert_track(&conn, &track).unwrap();

        increment_play_count(&conn, id, "2024-06-01T12:00:00Z").unwrap();
        let updated = get_track(&conn, id).unwrap().unwrap();
        assert_eq!(updated.play_count, 6); // was 5, now 6
        assert_eq!(updated.last_played.as_deref(), Some("2024-06-01T12:00:00Z"));
    }

    #[test]
    fn test_update_rating() {
        let conn = setup_db();
        let track = make_test_track("Test", "Artist", "Album");
        let id = insert_track(&conn, &track).unwrap();

        update_rating(&conn, id, 100).unwrap();
        let updated = get_track(&conn, id).unwrap().unwrap();
        assert_eq!(updated.rating, 100);
    }

    #[test]
    fn test_library_stats() {
        let conn = setup_db();
        let tracks = vec![
            make_test_track("A", "X", "Y"),
            make_test_track("B", "X", "Y"),
        ];
        insert_tracks_batch(&conn, &tracks).unwrap();

        let stats = get_library_stats(&conn).unwrap();
        assert_eq!(stats.total_tracks, 2);
        assert_eq!(stats.total_duration_ms, 480000); // 240000 * 2
        assert_eq!(stats.total_size_bytes, 10000000); // 5000000 * 2
    }

    #[test]
    fn test_get_distinct_values() {
        let conn = setup_db();
        let tracks = vec![
            make_test_track("A", "Rise Against", "Album"),
            make_test_track("B", "Metallica", "Album"),
            make_test_track("C", "Rise Against", "Album"),
        ];
        insert_tracks_batch(&conn, &tracks).unwrap();

        let artists = get_distinct_values(&conn, "artist").unwrap();
        assert_eq!(artists.len(), 2);
        assert!(artists.contains(&"Metallica".to_string()));
        assert!(artists.contains(&"Rise Against".to_string()));
    }
}
```

**Step 2: Run tests**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes/src-tauri
cargo test db::tracks
```

Expected: All 7 tests pass.

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add track CRUD operations with search, pagination, and stats"
```

---

### Task 5: Playlist CRUD Operations

**Files:**
- Modify: `src-tauri/src/db/playlists.rs`

**Step 1: Implement playlist operations with tests**

Replace `src-tauri/src/db/playlists.rs`:
```rust
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub persistent_id: Option<String>,
    pub is_smart: bool,
    pub is_folder: bool,
    pub parent_id: Option<i64>,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTreeNode {
    pub playlist: Playlist,
    pub children: Vec<PlaylistTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartPlaylistRule {
    pub id: i64,
    pub playlist_id: i64,
    pub match_all: bool,
    pub limit_enabled: bool,
    pub limit_value: Option<i64>,
    pub limit_type: Option<String>,
    pub limit_sort: Option<String>,
    pub live_updating: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartPlaylistCondition {
    pub id: i64,
    pub rule_id: i64,
    pub parent_group_id: Option<i64>,
    pub is_group: bool,
    pub group_match_all: bool,
    pub field: Option<String>,
    pub operator: Option<String>,
    pub value_text: Option<String>,
    pub value_int: Option<i64>,
    pub value_date: Option<String>,
    pub value_int2: Option<i64>,
    pub value_date2: Option<String>,
    pub value_units: Option<String>,
    pub position: i64,
}

/// Insert a playlist, returning its ID.
pub fn insert_playlist(conn: &Connection, playlist: &Playlist) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO playlists (name, persistent_id, is_smart, is_folder, parent_id, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            playlist.name,
            playlist.persistent_id,
            playlist.is_smart,
            playlist.is_folder,
            playlist.parent_id,
            playlist.sort_order,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Add a track to a playlist at a given position.
pub fn add_track_to_playlist(
    conn: &Connection,
    playlist_id: i64,
    track_id: i64,
    position: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        params![playlist_id, track_id, position],
    )?;
    Ok(())
}

/// Get all playlists as a flat list.
pub fn get_all_playlists(conn: &Connection) -> Result<Vec<Playlist>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, persistent_id, is_smart, is_folder, parent_id, sort_order
         FROM playlists ORDER BY sort_order ASC, name ASC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            name: row.get(1)?,
            persistent_id: row.get(2)?,
            is_smart: row.get(3)?,
            is_folder: row.get(4)?,
            parent_id: row.get(5)?,
            sort_order: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// Build a tree structure from flat playlist list.
pub fn build_playlist_tree(playlists: &[Playlist]) -> Vec<PlaylistTreeNode> {
    fn build_children(playlists: &[Playlist], parent_id: Option<i64>) -> Vec<PlaylistTreeNode> {
        playlists
            .iter()
            .filter(|p| p.parent_id == parent_id)
            .map(|p| PlaylistTreeNode {
                playlist: p.clone(),
                children: build_children(playlists, Some(p.id)),
            })
            .collect()
    }

    build_children(playlists, None)
}

/// Find a playlist by its iTunes Persistent ID.
pub fn find_playlist_by_persistent_id(
    conn: &Connection,
    persistent_id: &str,
) -> Result<Option<Playlist>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, persistent_id, is_smart, is_folder, parent_id, sort_order
         FROM playlists WHERE persistent_id = ?1"
    )?;
    let mut rows = stmt.query_map(params![persistent_id], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            name: row.get(1)?,
            persistent_id: row.get(2)?,
            is_smart: row.get(3)?,
            is_folder: row.get(4)?,
            parent_id: row.get(5)?,
            sort_order: row.get(6)?,
        })
    })?;
    match rows.next() {
        Some(Ok(p)) => Ok(Some(p)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Insert a smart playlist rule, returning its ID.
pub fn insert_smart_rule(conn: &Connection, rule: &SmartPlaylistRule) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO smart_playlist_rules (playlist_id, match_all, limit_enabled, limit_value, limit_type, limit_sort, live_updating)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            rule.playlist_id,
            rule.match_all,
            rule.limit_enabled,
            rule.limit_value,
            rule.limit_type,
            rule.limit_sort,
            rule.live_updating,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a smart playlist condition, returning its ID.
pub fn insert_smart_condition(
    conn: &Connection,
    condition: &SmartPlaylistCondition,
) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO smart_playlist_conditions (rule_id, parent_group_id, is_group, group_match_all, field, operator, value_text, value_int, value_date, value_int2, value_date2, value_units, position)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            condition.rule_id,
            condition.parent_group_id,
            condition.is_group,
            condition.group_match_all,
            condition.field,
            condition.operator,
            condition.value_text,
            condition.value_int,
            condition.value_date,
            condition.value_int2,
            condition.value_date2,
            condition.value_units,
            condition.position,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get the smart rule for a playlist.
pub fn get_smart_rule(conn: &Connection, playlist_id: i64) -> Result<Option<SmartPlaylistRule>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, playlist_id, match_all, limit_enabled, limit_value, limit_type, limit_sort, live_updating
         FROM smart_playlist_rules WHERE playlist_id = ?1"
    )?;
    let mut rows = stmt.query_map(params![playlist_id], |row| {
        Ok(SmartPlaylistRule {
            id: row.get(0)?,
            playlist_id: row.get(1)?,
            match_all: row.get(2)?,
            limit_enabled: row.get(3)?,
            limit_value: row.get(4)?,
            limit_type: row.get(5)?,
            limit_sort: row.get(6)?,
            live_updating: row.get(7)?,
        })
    })?;
    match rows.next() {
        Some(Ok(r)) => Ok(Some(r)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Get all conditions for a smart rule.
pub fn get_smart_conditions(conn: &Connection, rule_id: i64) -> Result<Vec<SmartPlaylistCondition>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, rule_id, parent_group_id, is_group, group_match_all, field, operator,
                value_text, value_int, value_date, value_int2, value_date2, value_units, position
         FROM smart_playlist_conditions WHERE rule_id = ?1 ORDER BY position ASC"
    )?;
    let rows = stmt.query_map(params![rule_id], |row| {
        Ok(SmartPlaylistCondition {
            id: row.get(0)?,
            rule_id: row.get(1)?,
            parent_group_id: row.get(2)?,
            is_group: row.get(3)?,
            group_match_all: row.get(4)?,
            field: row.get(5)?,
            operator: row.get(6)?,
            value_text: row.get(7)?,
            value_int: row.get(8)?,
            value_date: row.get(9)?,
            value_int2: row.get(10)?,
            value_date2: row.get(11)?,
            value_units: row.get(12)?,
            position: row.get(13)?,
        })
    })?;
    rows.collect()
}

/// Preferences helpers
pub fn get_preference(conn: &Connection, key: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT value FROM preferences WHERE key = ?1")?;
    let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

pub fn set_preference(conn: &Connection, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO preferences (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use crate::db::tracks::{insert_track, make_test_track};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_get_playlist() {
        let conn = setup_db();
        let playlist = Playlist {
            id: 0,
            name: "My Playlist".to_string(),
            persistent_id: Some("ABC123".to_string()),
            is_smart: false,
            is_folder: false,
            parent_id: None,
            sort_order: Some(1),
        };

        let id = insert_playlist(&conn, &playlist).unwrap();
        let all = get_all_playlists(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "My Playlist");
        assert_eq!(all[0].id, id);
    }

    #[test]
    fn test_playlist_tree() {
        let conn = setup_db();

        // Create folder
        let folder = Playlist {
            id: 0, name: "Rock".to_string(), persistent_id: Some("F1".to_string()),
            is_smart: false, is_folder: true, parent_id: None, sort_order: Some(1),
        };
        let folder_id = insert_playlist(&conn, &folder).unwrap();

        // Create child playlists
        let child1 = Playlist {
            id: 0, name: "Rise Against".to_string(), persistent_id: Some("C1".to_string()),
            is_smart: true, is_folder: false, parent_id: Some(folder_id), sort_order: Some(1),
        };
        let child2 = Playlist {
            id: 0, name: "Metallica".to_string(), persistent_id: Some("C2".to_string()),
            is_smart: true, is_folder: false, parent_id: Some(folder_id), sort_order: Some(2),
        };
        insert_playlist(&conn, &child1).unwrap();
        insert_playlist(&conn, &child2).unwrap();

        let all = get_all_playlists(&conn).unwrap();
        let tree = build_playlist_tree(&all);

        assert_eq!(tree.len(), 1); // One root folder
        assert_eq!(tree[0].playlist.name, "Rock");
        assert_eq!(tree[0].children.len(), 2);
    }

    #[test]
    fn test_playlist_tracks() {
        let conn = setup_db();

        // Insert some tracks
        let t1 = make_test_track("Song A", "Artist", "Album");
        let t2 = make_test_track("Song B", "Artist", "Album");
        let tid1 = insert_track(&conn, &t1).unwrap();
        let tid2 = insert_track(&conn, &t2).unwrap();

        // Create playlist and add tracks
        let playlist = Playlist {
            id: 0, name: "Test".to_string(), persistent_id: None,
            is_smart: false, is_folder: false, parent_id: None, sort_order: None,
        };
        let pid = insert_playlist(&conn, &playlist).unwrap();
        add_track_to_playlist(&conn, pid, tid1, 0).unwrap();
        add_track_to_playlist(&conn, pid, tid2, 1).unwrap();

        // Query tracks for this playlist
        let query = crate::db::tracks::TrackQuery {
            offset: 0, limit: 100, sort_column: None, sort_direction: None,
            search: None, genre: None, artist: None, album: None,
            playlist_id: Some(pid),
        };
        let results = crate::db::tracks::query_tracks(&conn, &query).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_smart_playlist_rule_and_conditions() {
        let conn = setup_db();

        let playlist = Playlist {
            id: 0, name: "Smart".to_string(), persistent_id: None,
            is_smart: true, is_folder: false, parent_id: None, sort_order: None,
        };
        let pid = insert_playlist(&conn, &playlist).unwrap();

        let rule = SmartPlaylistRule {
            id: 0, playlist_id: pid, match_all: true,
            limit_enabled: false, limit_value: None, limit_type: None,
            limit_sort: None, live_updating: true,
        };
        let rid = insert_smart_rule(&conn, &rule).unwrap();

        let cond = SmartPlaylistCondition {
            id: 0, rule_id: rid, parent_group_id: None,
            is_group: false, group_match_all: true,
            field: Some("artist".to_string()),
            operator: Some("is".to_string()),
            value_text: Some("Rise Against".to_string()),
            value_int: None, value_date: None, value_int2: None,
            value_date2: None, value_units: None, position: 0,
        };
        insert_smart_condition(&conn, &cond).unwrap();

        let fetched_rule = get_smart_rule(&conn, pid).unwrap().unwrap();
        assert!(fetched_rule.match_all);

        let conditions = get_smart_conditions(&conn, rid).unwrap();
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].field.as_deref(), Some("artist"));
        assert_eq!(conditions[0].value_text.as_deref(), Some("Rise Against"));
    }

    #[test]
    fn test_preferences() {
        let conn = setup_db();
        set_preference(&conn, "volume", "75").unwrap();
        assert_eq!(get_preference(&conn, "volume").unwrap(), Some("75".to_string()));

        set_preference(&conn, "volume", "50").unwrap();
        assert_eq!(get_preference(&conn, "volume").unwrap(), Some("50".to_string()));

        assert_eq!(get_preference(&conn, "nonexistent").unwrap(), None);
    }

    #[test]
    fn test_find_by_persistent_id() {
        let conn = setup_db();
        let playlist = Playlist {
            id: 0, name: "Test".to_string(), persistent_id: Some("XYZ789".to_string()),
            is_smart: false, is_folder: false, parent_id: None, sort_order: None,
        };
        insert_playlist(&conn, &playlist).unwrap();

        let found = find_playlist_by_persistent_id(&conn, "XYZ789").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Test");

        let not_found = find_playlist_by_persistent_id(&conn, "NOPE").unwrap();
        assert!(not_found.is_none());
    }
}
```

Note: The `make_test_track` function in tracks.rs needs to be made `pub` (and not `#[cfg(test)]`-gated) so that the playlist tests can use it. Alternatively, make it `pub(crate)` with `#[cfg(test)]`. Update tracks.rs to add `#[cfg(test)] pub(crate)` visibility on `make_test_track` at the module level (move it outside the `tests` module but keep it `#[cfg(test)]`):

Actually, the cleaner approach: put `make_test_track` in a shared test utils module. Create `src-tauri/src/db/test_utils.rs`:
```rust
#[cfg(test)]
pub fn make_test_track(title: &str, artist: &str, album: &str) -> super::tracks::Track {
    // ... same body as before
}
```

Or simply duplicate the helper in each test module. For now, keep it simple: in `tracks.rs`, move `make_test_track` to be `pub(crate)` outside the test module:

In `src-tauri/src/db/tracks.rs`, add just above `#[cfg(test)] mod tests`:
```rust
#[cfg(test)]
pub(crate) fn make_test_track(title: &str, artist: &str, album: &str) -> Track {
    // ... same body
}
```

And in the `tests` module, call it as `super::make_test_track(...)`.

**Step 2: Run tests**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes/src-tauri
cargo test db::playlists
```

Expected: All 6 tests pass.

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add playlist CRUD, smart playlist rules, preferences, and tree building"
```

---

## Phase 3: iTunes Import Core

### Task 6: Path Rewriter (Windows -> Linux)

**Files:**
- Create: `src-tauri/src/import/mod.rs`
- Create: `src-tauri/src/import/path_rewriter.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod import`)

**Step 1: Implement path rewriter with tests**

Create `src-tauri/src/import/mod.rs`:
```rust
pub mod path_rewriter;
pub mod itunes_xml;
pub mod smart_playlist;
```

Create `src-tauri/src/import/path_rewriter.rs`:
```rust
use percent_encoding::percent_decode_str;
use std::collections::HashMap;

/// Maps Windows drive letter prefixes to Linux mount points.
///
/// Example mapping: {"D:" -> "/run/media/joseph/Local Disk"}
pub struct PathRewriter {
    drive_mappings: HashMap<String, String>,
}

impl PathRewriter {
    pub fn new(mappings: HashMap<String, String>) -> Self {
        Self {
            drive_mappings: mappings,
        }
    }

    /// Rewrite an iTunes file:// URL to a Linux filesystem path.
    ///
    /// Input:  "file://localhost/D:/Users/Joseph/Music/iTunes/iTunes%20Media//Music/Rise%20Against/The%20Sufferer%20&#38;%20the%20Witness/10%20Behind%20Closed%20Doors.m4p"
    /// Output: "/run/media/joseph/Local Disk/Users/Joseph/Music/iTunes/iTunes Media/Music/Rise Against/The Sufferer & the Witness/10 Behind Closed Doors.m4p"
    pub fn rewrite(&self, itunes_location: &str) -> Result<String, PathRewriteError> {
        // Strip file://localhost/ prefix
        let path = itunes_location
            .strip_prefix("file://localhost/")
            .ok_or(PathRewriteError::InvalidPrefix(itunes_location.to_string()))?;

        // URL-decode percent-encoded characters
        let decoded = percent_decode_str(path)
            .decode_utf8()
            .map_err(|e| PathRewriteError::Utf8Error(e.to_string()))?
            .to_string();

        // Extract drive letter (e.g., "D:")
        let drive = if decoded.len() >= 2 && decoded.as_bytes()[1] == b':' {
            &decoded[..2]
        } else {
            return Err(PathRewriteError::NoDriveLetter(decoded));
        };

        // Look up Linux mount point
        let mount_point = self
            .drive_mappings
            .get(drive)
            .ok_or_else(|| PathRewriteError::UnmappedDrive(drive.to_string()))?;

        // Replace drive letter with mount point
        let rest = &decoded[2..]; // Everything after "D:"
        let linux_path = format!("{}{}", mount_point, rest);

        // Normalize double slashes to single
        let normalized = linux_path.replace("//", "/");

        Ok(normalized)
    }

    /// Scan a list of iTunes location URLs and return the set of drive letters found.
    pub fn detect_drive_letters(locations: &[String]) -> Vec<String> {
        let mut drives: Vec<String> = locations
            .iter()
            .filter_map(|loc| {
                let path = loc.strip_prefix("file://localhost/")?;
                let decoded = percent_decode_str(path).decode_utf8().ok()?;
                if decoded.len() >= 2 && decoded.as_bytes()[1] == b':' {
                    Some(decoded[..2].to_uppercase())
                } else {
                    None
                }
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        drives.sort();
        drives
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathRewriteError {
    #[error("invalid file:// prefix: {0}")]
    InvalidPrefix(String),
    #[error("UTF-8 decode error: {0}")]
    Utf8Error(String),
    #[error("no drive letter found: {0}")]
    NoDriveLetter(String),
    #[error("unmapped drive letter: {0}")]
    UnmappedDrive(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rewriter() -> PathRewriter {
        let mut mappings = HashMap::new();
        mappings.insert("D:".to_string(), "/run/media/joseph/Local Disk".to_string());
        mappings.insert("C:".to_string(), "/mnt/windows".to_string());
        PathRewriter::new(mappings)
    }

    #[test]
    fn test_basic_path_rewrite() {
        let rw = make_rewriter();
        let result = rw
            .rewrite("file://localhost/D:/Users/Joseph/Music/song.mp3")
            .unwrap();
        assert_eq!(
            result,
            "/run/media/joseph/Local Disk/Users/Joseph/Music/song.mp3"
        );
    }

    #[test]
    fn test_percent_decode_spaces() {
        let rw = make_rewriter();
        let result = rw
            .rewrite("file://localhost/D:/Users/Joseph/Music/iTunes/iTunes%20Media/Music/song.mp3")
            .unwrap();
        assert_eq!(
            result,
            "/run/media/joseph/Local Disk/Users/Joseph/Music/iTunes/iTunes Media/Music/song.mp3"
        );
    }

    #[test]
    fn test_double_slash_normalization() {
        let rw = make_rewriter();
        let result = rw
            .rewrite("file://localhost/D:/Users/Joseph/Music/iTunes/iTunes%20Media//Music/Rise%20Against/song.m4p")
            .unwrap();
        assert!(
            !result.contains("//"),
            "double slashes should be normalized: {}",
            result
        );
    }

    #[test]
    fn test_c_drive_mapping() {
        let rw = make_rewriter();
        let result = rw
            .rewrite("file://localhost/C:/Users/Joseph/file.mp3")
            .unwrap();
        assert_eq!(result, "/mnt/windows/Users/Joseph/file.mp3");
    }

    #[test]
    fn test_unmapped_drive_error() {
        let rw = make_rewriter();
        let result = rw.rewrite("file://localhost/E:/music/song.mp3");
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_drive_letters() {
        let locations = vec![
            "file://localhost/D:/Users/Music/a.mp3".to_string(),
            "file://localhost/D:/Users/Music/b.mp3".to_string(),
            "file://localhost/C:/Users/Music/c.mp3".to_string(),
        ];
        let drives = PathRewriter::detect_drive_letters(&locations);
        assert_eq!(drives, vec!["C:", "D:"]);
    }
}
```

Add `mod import;` to `src-tauri/src/lib.rs`.

**Step 2: Run tests**

```bash
cd /home/joseph/Projects/PegasusHeavyIndustries/tuxtunes/src-tauri
cargo test import::path_rewriter
```

Expected: All 6 tests pass.

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add iTunes path rewriter with drive letter mapping and URL decoding"
```

---

### Task 7: iTunes XML Parser

**Files:**
- Modify: `src-tauri/src/import/itunes_xml.rs`

This task parses the iTunes Music Library.xml (plist format) and extracts tracks and playlists. The `plist` crate handles the XML plist format natively -- we parse into `plist::Value` and extract fields.

**Step 1: Implement iTunes XML parser**

Create `src-tauri/src/import/itunes_xml.rs`. This is a large file. Key functionality:

- `parse_itunes_library(path: &Path) -> Result<ItunesLibrary>` -- parses the XML file
- `ItunesLibrary` contains `tracks: HashMap<i64, ItunesTrack>` and `playlists: Vec<ItunesPlaylist>`
- `ItunesTrack` maps directly from the plist dict
- `ItunesPlaylist` contains name, persistent ID, track IDs, folder flag, parent persistent ID, smart info/criteria blobs

The implementation should use `plist::Value::from_file()` to parse the XML, then walk the nested dict structure to extract tracks and playlists.

**Important considerations:**
- The XML file is 89MB. `plist::Value::from_file()` loads it all into memory -- this is fine, 89MB is manageable.
- Track keys in the plist use string representations of integers (e.g., `"5882"`)
- Location field is a URL-encoded `file://localhost/` path
- Smart playlists have `Smart Info` and `Smart Criteria` as base64-encoded `<data>` blobs

The parser should extract raw data only -- path rewriting and smart playlist decoding happen in later stages.

Write comprehensive tests using small inline plist XML strings.

**Step 2: Run tests**

```bash
cargo test import::itunes_xml
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add iTunes XML plist parser for tracks and playlists"
```

---

### Task 8: Smart Playlist Binary Decoder

**Files:**
- Modify: `src-tauri/src/import/smart_playlist.rs`

This is the most technically complex task. We decode the proprietary iTunes smart playlist binary format.

**Step 1: Implement the binary decoder**

Create `src-tauri/src/import/smart_playlist.rs`. Key structures:

```rust
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Decoded smart playlist info (from "Smart Info" blob)
pub struct SmartInfo {
    pub live_updating: bool,
    pub match_all: bool,
    pub limit_enabled: bool,
    pub limit_value: u32,
    pub limit_type: LimitType,
    pub limit_sort: LimitSort,
    pub reverse_sort: bool,
}

/// Decoded smart playlist criteria (from "Smart Criteria" blob)
pub struct SmartCriteria {
    pub rules: Vec<SmartRule>,
}

pub enum SmartRule {
    Condition {
        field: SmartField,
        operator: SmartOperator,
        value: SmartValue,
    },
    Group {
        match_all: bool,
        rules: Vec<SmartRule>,
    },
}
```

The decoder reads:
1. **Smart Info** (92 bytes): byte 1 = live updating flag, bytes at various offsets contain limit settings
2. **Smart Criteria**: starts with `SLst` magic (4 bytes), then header with version, match-all flag, rule count. Each rule is a fixed-size structure with field code, operator code, and value data.

Field codes, operator codes, and their mappings are defined as enums based on the reverse-engineered format documented in the design doc.

Test with base64-encoded blobs extracted from the actual iTunes library XML (samples captured during exploration).

**Step 2: Run tests**

```bash
cargo test import::smart_playlist
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add iTunes smart playlist binary format decoder"
```

---

### Task 9: Smart Playlist SQL Evaluator

**Files:**
- Create: `src-tauri/src/db/smart_eval.rs`
- Modify: `src-tauri/src/db/mod.rs`

This generates SQL WHERE clauses from smart playlist conditions stored in the database.

**Step 1: Implement SQL generator**

Create `src-tauri/src/db/smart_eval.rs`:

Key function: `evaluate_smart_playlist(conn: &Connection, playlist_id: i64, query: &TrackQuery) -> Result<Vec<TrackSummary>>`

This:
1. Loads the `smart_playlist_rules` and `smart_playlist_conditions` for the playlist
2. Recursively builds a SQL WHERE clause from the condition tree
3. Handles all operator types (is, contains, starts_with, greater_than, in_the_last, in_the_range, etc.)
4. Applies limit and sort settings from the rule
5. Handles playlist-references-playlist via subquery

Field-to-column mapping:
```rust
fn field_to_column(field: &str) -> &str {
    match field {
        "title" => "title",
        "artist" => "artist",
        "album" => "album",
        "album_artist" => "album_artist",
        "composer" => "composer",
        "genre" => "genre",
        "grouping" => "grouping",
        "comment" => "comment",
        "year" => "year",
        "track_number" => "track_number",
        "disc_number" => "disc_number",
        "bpm" => "bpm",
        "duration_ms" => "duration_ms",
        "size_bytes" => "size_bytes",
        "bit_rate" => "bit_rate",
        "sample_rate" => "sample_rate",
        "kind" => "kind",
        "rating" => "rating",
        "play_count" => "play_count",
        "skip_count" => "skip_count",
        "last_played" => "last_played",
        "last_skipped" => "last_skipped",
        "date_added" => "date_added",
        "date_modified" => "date_modified",
        "compilation" => "compilation",
        _ => "title",
    }
}
```

Operator-to-SQL mapping handles parameterized queries to prevent injection.

Test with various condition combinations: single condition, AND group, OR group, nested groups, in_the_last date operator, range operator, playlist reference.

Add `pub mod smart_eval;` to `src-tauri/src/db/mod.rs`.

**Step 2: Run tests**

```bash
cargo test db::smart_eval
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add smart playlist SQL evaluator with full operator support"
```

---

### Task 10: Import Orchestrator

**Files:**
- Create: `src-tauri/src/import/orchestrator.rs`
- Modify: `src-tauri/src/import/mod.rs`

This ties together the XML parser, path rewriter, smart playlist decoder, and database layer.

**Step 1: Implement import orchestrator**

Create `src-tauri/src/import/orchestrator.rs`:

```rust
use std::path::Path;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ImportProgress {
    pub phase: String,       // "parsing", "tracks", "playlists", "verifying"
    pub current: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub tracks_imported: u64,
    pub tracks_skipped: u64,
    pub playlists_imported: u64,
    pub smart_playlists_imported: u64,
    pub folders_imported: u64,
    pub errors: Vec<String>,
    pub missing_files: Vec<String>,
}

pub fn run_import(
    app_handle: &AppHandle,
    xml_path: &Path,
    drive_mappings: HashMap<String, String>,
    db: &crate::db::Database,
) -> Result<ImportResult, String> {
    // 1. Parse iTunes XML
    // 2. Rewrite paths
    // 3. Insert tracks in batches
    // 4. Insert folders
    // 5. Insert playlists (resolve parent IDs via persistent ID)
    // 6. Decode and insert smart playlists
    // 7. Verify file existence
    // 8. Emit progress events throughout
    todo!()
}
```

The orchestrator:
1. Emits `import-progress` Tauri events throughout so the frontend can show a progress bar
2. Processes tracks in batches of 1000 for SQLite transaction efficiency
3. Builds a mapping of `iTunes Track ID -> local DB ID` for playlist track resolution
4. Builds a mapping of `iTunes Persistent ID -> local DB ID` for folder hierarchy resolution
5. Handles errors gracefully (logs and continues, doesn't abort on single track failure)

Add `pub mod orchestrator;` to `src-tauri/src/import/mod.rs`.

**Step 2: Run tests (integration test with small fixture)**

```bash
cargo test import::orchestrator
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add import orchestrator tying XML parsing, path rewriting, and DB insertion"
```

---

## Phase 4: Playback Engine

### Task 11: libmpv Integration

**Files:**
- Create: `src-tauri/src/playback/mod.rs`
- Create: `src-tauri/src/playback/mpv.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod playback`)
- Modify: `src-tauri/Cargo.toml` (add mpv dependency)

**Step 1: Add mpv dependency**

Research which mpv crate compiles on the current system. Try `libmpv` first:

```bash
# Check if libmpv is installed
pkg-config --libs mpv
```

Add to `src-tauri/Cargo.toml`:
```toml
libmpv2 = "4"  # or whichever version is current
```

If `libmpv2` doesn't exist or doesn't compile, fall back to raw FFI bindings or `mpv` crate. The exact crate may need adjustment at implementation time.

**Step 2: Implement player wrapper**

Create `src-tauri/src/playback/mod.rs`:
```rust
pub mod mpv;
```

Create `src-tauri/src/playback/mpv.rs`:

The `Player` struct wraps libmpv and provides:
- `new()` -> initialize mpv with audio-only settings
- `play(path: &str)` -> load and play a file
- `pause()` / `resume()` / `toggle_pause()`
- `stop()`
- `seek(position_secs: f64)`
- `set_volume(volume: i64)` (0-100)
- `get_position() -> Option<f64>` (seconds)
- `get_duration() -> Option<f64>` (seconds)
- `is_paused() -> bool`

The player runs mpv's event loop on a background thread and sends state changes back via a channel. The Tauri setup code will spawn a thread that reads from this channel and emits Tauri events.

**Step 3: Implement play queue**

Add to `src-tauri/src/playback/mpv.rs`:

```rust
pub struct PlayQueue {
    tracks: Vec<QueuedTrack>,
    current_index: Option<usize>,
}

pub struct QueuedTrack {
    pub track_id: i64,
    pub file_path: String,
}
```

Queue operations: `set_queue()`, `next()`, `previous()`, `current()`, `shuffle()`.

**Step 4: Verify mpv initializes**

```bash
cargo test playback
```

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add libmpv playback engine with play queue"
```

---

## Phase 5: Tauri Command Layer

### Task 12: Tauri Commands -- Library, Playback, Playlists, Import

**Files:**
- Create: `src-tauri/src/commands/mod.rs`
- Create: `src-tauri/src/commands/library.rs`
- Create: `src-tauri/src/commands/playback.rs`
- Create: `src-tauri/src/commands/playlist.rs`
- Create: `src-tauri/src/commands/import.rs`
- Modify: `src-tauri/src/lib.rs` (register commands)
- Modify: `src-tauri/capabilities/default.json` (allowlist)

**Step 1: Create command modules**

`src-tauri/src/commands/mod.rs`:
```rust
pub mod library;
pub mod playback;
pub mod playlist;
pub mod import;
```

`src-tauri/src/commands/library.rs`:
```rust
use tauri::State;
use crate::db::Database;
use crate::db::tracks::{TrackQuery, TrackSummary, Track, LibraryStats};

#[tauri::command]
pub fn get_tracks(db: State<Database>, query: TrackQuery) -> Result<Vec<TrackSummary>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::query_tracks(&conn, &query).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_track_count(db: State<Database>, query: TrackQuery) -> Result<i64, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::count_tracks(&conn, &query).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_track_detail(db: State<Database>, id: i64) -> Result<Option<Track>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::get_track(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_library_stats(db: State<Database>) -> Result<LibraryStats, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::get_library_stats(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_distinct_values(db: State<Database>, column: String) -> Result<Vec<String>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::get_distinct_values(&conn, &column).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_track_rating(db: State<Database>, track_id: i64, rating: i64) -> Result<(), String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    crate::db::tracks::update_rating(&conn, track_id, rating).map_err(|e| e.to_string())
}
```

`src-tauri/src/commands/playback.rs`:
```rust
use tauri::State;
use crate::playback::mpv::Player;

#[tauri::command]
pub fn play_track(player: State<Player>, file_path: String) -> Result<(), String> {
    player.play(&file_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pause_playback(player: State<Player>) -> Result<(), String> {
    player.toggle_pause().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stop_playback(player: State<Player>) -> Result<(), String> {
    player.stop().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn seek_to(player: State<Player>, position_secs: f64) -> Result<(), String> {
    player.seek(position_secs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_volume(player: State<Player>, volume: i64) -> Result<(), String> {
    player.set_volume(volume).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_playback_state(player: State<Player>) -> Result<PlaybackState, String> {
    // Return current position, duration, paused state, current track info
    todo!()
}
```

`src-tauri/src/commands/playlist.rs` -- wraps playlist DB operations.

`src-tauri/src/commands/import.rs` -- wraps the import orchestrator, spawning it on a background thread.

**Step 2: Register all commands in lib.rs**

Update the `tauri::Builder` in `src-tauri/src/lib.rs`:
```rust
.invoke_handler(tauri::generate_handler![
    commands::library::get_tracks,
    commands::library::get_track_count,
    commands::library::get_track_detail,
    commands::library::get_library_stats,
    commands::library::get_distinct_values,
    commands::library::update_track_rating,
    commands::playback::play_track,
    commands::playback::pause_playback,
    commands::playback::stop_playback,
    commands::playback::seek_to,
    commands::playback::set_volume,
    commands::playlist::get_playlist_tree,
    commands::playlist::get_smart_playlist_tracks,
    commands::import::start_import,
    commands::import::detect_drives,
])
```

**Step 3: Update capabilities/default.json**

Tauri 2 requires commands to be allowlisted in capabilities.

**Step 4: Verify compilation**

```bash
cargo check
```

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add Tauri command layer for library, playback, playlists, and import"
```

---

## Phase 6: Angular Frontend

### Task 13: TypeScript Models and Tauri Service

**Files:**
- Create: `src/app/models/track.model.ts`
- Create: `src/app/models/playlist.model.ts`
- Create: `src/app/models/smart-rule.model.ts`
- Create: `src/app/services/tauri.service.ts`

**Step 1: Define TypeScript interfaces matching Rust structs**

`src/app/models/track.model.ts`:
```typescript
export interface TrackSummary {
  id: number;
  title: string | null;
  artist: string | null;
  album: string | null;
  albumArtist: string | null;
  genre: string | null;
  durationMs: number | null;
  trackNumber: number | null;
  discNumber: number | null;
  year: number | null;
  rating: number;
  playCount: number;
  dateAdded: string;
  filePath: string;
}

export interface Track extends TrackSummary {
  persistentId: string | null;
  composer: string | null;
  // ... all fields
}

export interface TrackQuery {
  offset: number;
  limit: number;
  sortColumn?: string;
  sortDirection?: string;
  search?: string;
  genre?: string;
  artist?: string;
  album?: string;
  playlistId?: number;
}

export interface LibraryStats {
  totalTracks: number;
  totalDurationMs: number;
  totalSizeBytes: number;
}
```

`src/app/models/playlist.model.ts`:
```typescript
export interface Playlist {
  id: number;
  name: string;
  persistentId: string | null;
  isSmart: boolean;
  isFolder: boolean;
  parentId: number | null;
  sortOrder: number | null;
}

export interface PlaylistTreeNode {
  playlist: Playlist;
  children: PlaylistTreeNode[];
}
```

**Step 2: Create Tauri service wrapper**

`src/app/services/tauri.service.ts`:
```typescript
import { Injectable } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';

@Injectable({ providedIn: 'root' })
export class TauriService {
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    return invoke<T>(command, args);
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
    return listen<T>(event, (e) => handler(e.payload));
  }
}
```

**Step 3: Create domain services**

`src/app/services/library.service.ts` -- wraps track queries
`src/app/services/playback.service.ts` -- wraps playback commands + listens for events
`src/app/services/playlist.service.ts` -- wraps playlist queries

Each service uses Angular signals for reactive state.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add Angular TypeScript models and Tauri service layer"
```

---

### Task 14: App Shell and Layout

**Files:**
- Modify: `src/app/app.component.ts`
- Modify: `src/app/app.component.html`
- Create: `src/app/components/transport/transport.component.ts`
- Create: `src/app/components/sidebar/sidebar.component.ts`
- Create: `src/app/components/track-list/track-list.component.ts`
- Create: `src/app/components/status-bar/status-bar.component.ts`

**Step 1: Build the main app shell**

The app shell is a CSS Grid layout:
```
grid-template-rows: auto 1fr auto
grid-template-columns: 250px 1fr
```

- Row 1: Transport bar (spans both columns)
- Row 2: Sidebar | Track list
- Row 3: Status bar (spans both columns)

Use Tailwind classes for all styling.

**Step 2: Create stub components**

Each component starts as a minimal placeholder with the right CSS dimensions:
- `transport`: h-20 bar at top with accent color border-bottom
- `sidebar`: w-64 bg-bg-secondary with overflow-y-auto
- `track-list`: flex-1 with placeholder text
- `status-bar`: h-8 bar at bottom

**Step 3: Verify layout renders**

```bash
npm run start
# Open http://localhost:4200 in browser
```

Expected: Dark-themed 3-row layout with sidebar visible.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add app shell layout with transport, sidebar, track list, and status bar"
```

---

### Task 15: Transport Bar Component

**Files:**
- Modify: `src/app/components/transport/transport.component.ts`
- Modify: `src/app/components/transport/transport.component.html`

**Step 1: Implement transport controls**

The transport bar includes:
- Previous/Play-Pause/Next buttons (SVG icons)
- Seek slider (HTML range input styled with Tailwind)
- Current time / total time display
- Volume slider
- Now playing text (artist - title)

Uses `PlaybackService` signals for reactive state:
```typescript
currentTrack = this.playbackService.currentTrack;
isPlaying = this.playbackService.isPlaying;
currentTime = this.playbackService.currentTime;
duration = this.playbackService.duration;
volume = this.playbackService.volume;
```

**Step 2: Wire up button click handlers to Tauri invoke**

```typescript
async onPlayPause() {
  await this.playbackService.togglePause();
}

async onSeek(event: Event) {
  const value = (event.target as HTMLInputElement).valueAsNumber;
  await this.playbackService.seekTo(value);
}
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add transport bar with playback controls, seek, and volume"
```

---

### Task 16: Sidebar Component

**Files:**
- Modify: `src/app/components/sidebar/sidebar.component.ts`
- Modify: `src/app/components/sidebar/sidebar.component.html`

**Step 1: Implement sidebar with library navigation and playlist tree**

Sidebar has two sections:
1. **Library navigation**: All Songs, Artists, Albums, Genres (static list items)
2. **Playlists**: Collapsible folder tree loaded from `PlaylistService`

Uses recursive `@for` template for nested folders:
```html
@for (node of playlistTree(); track node.playlist.id) {
  <div class="pl-2">
    @if (node.playlist.isFolder) {
      <button (click)="toggleFolder(node.playlist.id)">
        {{ isExpanded(node.playlist.id) ? '▾' : '▸' }}
        {{ node.playlist.name }}
      </button>
      @if (isExpanded(node.playlist.id)) {
        <!-- recursive children -->
      }
    } @else {
      <button (click)="selectPlaylist(node.playlist)">
        {{ node.playlist.name }}
      </button>
    }
  </div>
}
```

**Step 2: Load playlist tree on init**

```typescript
async ngOnInit() {
  const tree = await this.playlistService.getPlaylistTree();
  this.playlistTree.set(tree);
}
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add sidebar with library navigation and collapsible playlist tree"
```

---

### Task 17: Track List with Virtual Scrolling

**Files:**
- Modify: `src/app/components/track-list/track-list.component.ts`
- Modify: `src/app/components/track-list/track-list.component.html`

**Step 1: Set up CDK virtual scroll viewport**

This is the most performance-critical frontend component. With 347K tracks, we need virtual scrolling.

```typescript
import { CdkVirtualScrollViewport, CdkFixedSizeVirtualScroll, CdkVirtualForOf } from '@angular/cdk/scrolling';

@Component({
  imports: [CdkVirtualScrollViewport, CdkFixedSizeVirtualScroll, CdkVirtualForOf],
  template: `
    <cdk-virtual-scroll-viewport [itemSize]="36" class="h-full">
      <div *cdkVirtualFor="let track of dataSource" class="flex items-center h-9 border-b border-border hover:bg-bg-hover">
        <!-- track columns -->
      </div>
    </cdk-virtual-scroll-viewport>
  `
})
```

**Step 2: Implement custom DataSource**

Create a custom `DataSource` that fetches pages from the Rust backend:
```typescript
class TrackDataSource extends DataSource<TrackSummary> {
  // Fetches pages of tracks on scroll
  // Caches loaded pages
  // Supports sort/filter changes (resets cache)
}
```

**Step 3: Add sortable column headers**

```html
<div class="flex bg-bg-secondary border-b border-border text-text-secondary text-sm">
  <button (click)="sortBy('title')" class="flex-1 px-2 py-1 text-left">Title</button>
  <button (click)="sortBy('artist')" class="w-48 px-2 py-1 text-left">Artist</button>
  <button (click)="sortBy('album')" class="w-48 px-2 py-1 text-left">Album</button>
  <div class="w-16 px-2 py-1 text-right">Time</div>
  <div class="w-20 px-2 py-1 text-center">Rating</div>
</div>
```

**Step 4: Add search bar**

```html
<input type="text"
  placeholder="Search..."
  (input)="onSearch($event)"
  class="bg-bg-tertiary text-text-primary border border-border rounded px-3 py-1"
/>
```

Search debounces 300ms then reloads the DataSource with the new filter.

**Step 5: Add double-click-to-play**

```typescript
onTrackDoubleClick(track: TrackSummary) {
  this.playbackService.playTrack(track);
}
```

**Step 6: Commit**

```bash
git add -A
git commit -m "feat: add virtual-scrolled track list with sorting, search, and double-click-to-play"
```

---

### Task 18: Status Bar Component

**Files:**
- Create: `src/app/components/status-bar/status-bar.component.ts`

**Step 1: Implement status bar**

Shows library statistics from `LibraryService`:
```
42,313 songs | 125.3 days | 198.5 GB
```

Format duration as days/hours, size as GB/TB.

**Step 2: Commit**

```bash
git add -A
git commit -m "feat: add status bar with library statistics"
```

---

### Task 19: Import Wizard Component

**Files:**
- Create: `src/app/components/import-wizard/import-wizard.component.ts`
- Create: `src/app/components/import-wizard/import-wizard.component.html`

**Step 1: Build multi-step wizard**

Steps:
1. **File selection**: Open native file dialog via Tauri dialog plugin to select XML file
2. **Drive mapping**: Show detected drive letters, user maps each to a Linux path (text input + browse button) or selects "Copy" or "Skip"
3. **Import progress**: Progress bar fed by `import-progress` Tauri event listener
4. **Complete**: Show import results (tracks imported, errors, missing files)

**Step 2: Wire up to Tauri commands**

```typescript
async startImport() {
  const unlisten = await this.tauriService.listen<ImportProgress>('import-progress', (p) => {
    this.progress.set(p);
  });

  const result = await this.tauriService.invoke<ImportResult>('start_import', {
    xmlPath: this.selectedFile(),
    driveMappings: this.driveMappings(),
  });

  unlisten();
  this.result.set(result);
  this.step.set('complete');
}
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add import wizard with file picker, drive mapping, and progress tracking"
```

---

### Task 20: Smart Playlist Editor Component

**Files:**
- Create: `src/app/components/smart-playlist-editor/smart-playlist-editor.component.ts`
- Create: `src/app/components/smart-playlist-editor/smart-playlist-editor.component.html`

**Step 1: Build rule editor UI**

The smart playlist editor allows:
- Viewing current rules of a smart playlist
- Adding/removing conditions
- Choosing field (dropdown), operator (dropdown), value (text/number/date input)
- Toggling match all/any
- Adding nested groups
- Setting limit and sort options

**Step 2: Implement field/operator dropdowns**

Define available fields and their valid operators:
```typescript
const FIELD_CONFIG: Record<string, FieldConfig> = {
  artist: { type: 'string', label: 'Artist' },
  album: { type: 'string', label: 'Album' },
  genre: { type: 'string', label: 'Genre' },
  year: { type: 'number', label: 'Year' },
  rating: { type: 'number', label: 'Rating' },
  playCount: { type: 'number', label: 'Play Count' },
  dateAdded: { type: 'date', label: 'Date Added' },
  lastPlayed: { type: 'date', label: 'Last Played' },
  // ... all fields
};
```

String fields get: is, is not, contains, does not contain, starts with, ends with
Number fields get: is, is not, greater than, less than, in the range
Date fields get: is, is not, is before, is after, in the last, not in the last

**Step 3: Save rules via Tauri command**

On save, serialize the condition tree and send to Rust backend.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add smart playlist editor with full rule builder UI"
```

---

## Phase 7: Integration and Polish

### Task 21: Wire Up Playback Events

**Files:**
- Modify: `src/app/services/playback.service.ts`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Set up Tauri event emission from Rust**

In the Tauri `setup` closure, spawn a thread that:
- Polls mpv for position updates every 500ms
- Emits `position-update` events with `{ position, duration, paused }`
- On track end, emits `track-ended` and auto-advances the queue
- On track change, emits `track-changed` with track info
- Checks play-count threshold (50% or 30s) and increments in DB

**Step 2: Subscribe in Angular PlaybackService**

```typescript
constructor() {
  this.setupEventListeners();
}

private async setupEventListeners() {
  await listen<PositionUpdate>('position-update', (e) => {
    this.currentTime.set(e.payload.position);
    this.duration.set(e.payload.duration);
    this.isPlaying.set(!e.payload.paused);
  });

  await listen<TrackChanged>('track-changed', (e) => {
    this.currentTrack.set(e.payload);
  });
}
```

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: wire up mpv playback events to Angular via Tauri event bridge"
```

---

### Task 22: Preferences System

**Files:**
- Create: `src/app/services/preferences.service.ts`
- Modify: `src-tauri/src/commands/library.rs` (add preference commands)

**Step 1: Add preference Tauri commands**

```rust
#[tauri::command]
pub fn get_preference(db: State<Database>, key: String) -> Result<Option<String>, String> { ... }

#[tauri::command]
pub fn set_preference(db: State<Database>, key: String, value: String) -> Result<(), String> { ... }
```

**Step 2: Create Angular PreferencesService**

Saves/restores: volume, last played track, sidebar width, sort settings.

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add preferences system with save/restore across sessions"
```

---

### Task 23: Final Integration Testing

**Step 1: Test full import flow**

Using the actual iTunes library at `/run/media/joseph/Local Disk/Users/Joseph/Music/iTunes/iTunes Music Library.xml`:
- Run import wizard
- Map D: to `/run/media/joseph/Local Disk`
- Verify tracks appear in track list
- Verify playlist tree shows 9 genre folders with smart playlists nested inside
- Verify smart playlist evaluation returns correct tracks

**Step 2: Test playback**

- Double-click a track
- Verify audio plays
- Verify seek bar updates
- Verify play count increments after threshold

**Step 3: Test smart playlists**

- Click on an artist smart playlist (e.g., "Rise Against")
- Verify it shows only Rise Against tracks
- Edit the smart playlist rules
- Verify changes take effect

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat: complete TuxTunes v0.1.0 - iTunes clone with full import and smart playlists"
```

---

## Dependency Graph

```
Task 1 (Scaffold) ──────────────────────────────────────────────┐
Task 2 (Tailwind) ─────────────────────────────────────────────┐│
Task 3 (Schema) ──┬── Task 4 (Track CRUD) ──┐                 ││
                  └── Task 5 (Playlist CRUD) ┤                 ││
Task 6 (Path Rewriter) ──┐                   │                 ││
Task 7 (XML Parser) ─────┤                   │                 ││
Task 8 (Smart Decoder) ──┤                   │                 ││
                         └── Task 10 (Orchestrator)             ││
Task 4 + Task 5 ──── Task 9 (Smart Eval) ──┐                  ││
Task 11 (libmpv) ──────────────────────────┐│                  ││
                                           ││                  ││
Tasks 4,5,9,10,11 ─── Task 12 (Commands) ─┤│                  ││
                                           ││                  ││
Tasks 1,2 ─── Task 13 (TS Models) ────────┤│                  ││
              Task 14 (App Shell) ──┬──────┤│                  ││
              Task 15 (Transport) ──┤      ││                  ││
              Task 16 (Sidebar) ────┤      ││                  ││
              Task 17 (Track List) ─┤      ││                  ││
              Task 18 (Status Bar) ─┤      ││                  ││
              Task 19 (Import Wiz) ─┤      ││                  ││
              Task 20 (Smart Edit) ─┘      ││                  ││
                                           ││                  ││
Tasks 12-20 ──── Task 21 (Events) ────────┘│                  ││
                 Task 22 (Preferences) ─────┘                  ││
                 Task 23 (Integration) ────────────────────────┘│
```

## Notes for Implementor

1. **Rust edition**: The existing Cargo.toml says `edition = "2024"`. Tauri 2 needs `edition = "2021"`. The scaffold step will fix this.

2. **libmpv availability**: Run `pkg-config --libs mpv` before Task 11. If not installed: `sudo pacman -S mpv` (Arch).

3. **plist crate memory**: The 89MB XML file will use ~500MB RAM when parsed into `plist::Value`. This is acceptable for a desktop app with typical 8-16GB RAM.

4. **Smart playlist binary format**: The decoder in Task 8 is the highest-risk task. Use the Python `itunessmart` library as reference for expected outputs. Test against real blobs from the user's library.

5. **Virtual scrolling**: The `CdkVirtualScrollViewport` with a custom DataSource is critical for the 42K+ track library. Do not attempt to load all tracks into an array.

6. **Tauri argument naming**: JavaScript sends `camelCase`, Rust receives `snake_case`. Tauri auto-converts. But serde field names in Rust structs must be snake_case.

7. **Protected M4P files**: 720 tracks in the library are DRM-protected. libmpv cannot play these. Flag them in the UI (grey out or show a lock icon).
