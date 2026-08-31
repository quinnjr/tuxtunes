<h1 align="center">TuxTunes</h1>

<p align="center">
  <strong>A desktop music library manager and player for Linux — an iTunes replacement that takes your iTunes library with you.</strong>
</p>

<p align="center">
  <a href="https://github.com/quinnjr/tuxtunes/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/quinnjr/tuxtunes/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-Linux-informational">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2.x-24C8DB">
  <img alt="Angular" src="https://img.shields.io/badge/Angular-22-DD0031">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2021-000000">
  <img alt="License" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
</p>

---

TuxTunes is a native desktop app built on **Tauri 2** (Rust backend) and **Angular 22**
(zoneless, signals, standalone components), with **mpv** driving playback and **SQLite**
holding the library. It reads a real iTunes `.itl` database — tracks, playlists, folder
hierarchy, smart-playlist rules, play counts, ratings — and reconciles it into a local
library that keeps working after iTunes is gone.

## Highlights

**Library**

- Imports iTunes `.itl` databases directly (via [`itl-rs`](https://crates.io/crates/itl-rs)) —
  no XML export needed — including smart-playlist criteria decoded from the binary format.
- Repeatable **reconciliation**, not a one-shot import: re-run a sync source and TuxTunes
  diffs tracks and playlists (insert / update / delete) instead of duplicating your library.
- **Path remapping** rewrites Windows/macOS media paths (`--map FROM=TO`) onto your Linux
  filesystem, with relink and verify passes for anything that moved.
- Tag reading via [`lofty`](https://crates.io/crates/lofty), embedded + folder artwork
  extraction, and content hashing for duplicate and move detection.
- Optional **managed library**: files are organized on disk from a template scheme —
  `{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}` by default — and re-filed
  automatically when metadata changes (`keep_organized`).

**Playback**

- [libmpv](https://mpv.io/) backend: broad codec support, **gapless playback** via
  next-track prefetch, ReplayGain (off / track / album), device selection and exclusive mode.
- Play counts, skip counts, and last-played timestamps tracked the way iTunes did —
  a finished track counts as a play, an early skip counts as a skip.
- Volume persisted across launches.

**Smart playlists**

- Full rule engine over 26 fields (text / int / bool / date) with iTunes' operator set:
  `is`, `is not`, `contains`, `starts with`, `ends with`, `greater`, `less`, `in range`,
  `in the last`, `not in the last`, plus nested any/all groups.
- Live preview: see the matching track count while you build the rule, before saving.

**Desktop integration**

- **MPRIS2** — play/pause/next/previous and metadata from your DE, media keys, and
  panel applets.
- System tray, desktop notifications on track change, light/dark/system theming that
  follows `prefers-color-scheme`.
- Multiple library views: track list, album grid, artist split view, and a
  multi-column browser; context menus throughout (playlist management, reveal in file
  manager, move to trash).

## Install

### Arch Linux (AUR-style PKGBUILD)

```bash
git clone https://github.com/quinnjr/tuxtunes.git
cd tuxtunes/packaging
makepkg -si
```

Installs `tuxtunes` (GUI) and `tuxtunes-cli` (headless sync), plus a desktop entry and
icons. To build from a local checkout instead of cloning again:

```bash
TUXTUNES_SRC=file:///path/to/tuxtunes makepkg -si
```

### Runtime dependencies

`webkit2gtk-4.1` · `gtk3` · `libayatana-appindicator` · `mpv` · `sqlite` · `dbus` ·
`openssl` · `xdg-utils`

## Build from source

**Toolchain:** Rust (stable, 2021 edition) · Node.js 22+ · pnpm 11+ ·
`libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `libayatana-appindicator3-dev`,
`libssl-dev`, `libmpv-dev`, `pkg-config`

> This repo is **pnpm-only** — `npm install` will not produce a working tree.

```bash
pnpm install
cargo install prax-typegen --version '^0.8.2'   # once
pnpm run codegen                                # schema.prax → TS + Zod models
pnpm exec tauri dev                             # run the app
pnpm exec tauri build                           # release bundle
```

`tauri dev` starts the Angular dev server on **:4300** and launches the shell against it.
For frontend-only work, `pnpm start` serves the UI alone (Tauri `invoke` calls will fail).

## `tuxtunes-cli`

Headless management of iTunes sync sources — useful for scripting, cron, or a first
import before you ever open the GUI.

```bash
# Point at an iTunes library and remap its media paths
tuxtunes-cli source add \
  --name "Old Windows library" \
  --map 'C:\Users\me\Music=/home/me/Music' \
  /mnt/windows/Users/me/Music/iTunes/iTunes\ Library.itl

tuxtunes-cli source list
tuxtunes-cli sync run --all      # or: sync run <id>
tuxtunes-cli source remove <id>
```

Operates on the desktop app's database by default
(`$XDG_DATA_HOME/dev.quinnjr.tuxtunes/tuxtunes.db`); override with `--db <path>`.
Progress, warnings, and a per-source `tracks +N ~N -N, playlists +N ~N -N` summary go to
stderr.

## Architecture

```
Angular 22 (zoneless, signals)            Rust / Tauri 2
──────────────────────────────            ──────────────────────────────
library.service    ──── invoke() ───────► commands/{library,playback,
playback.service                           audio,sync,smart,playlists,
sync.service                               preferences}.rs
preferences.service                                  │
theme / ui / context-menu                            ▼
                                          db/      (SQLite via Prax ORM)
                                          fs/      (ingest, organize, relink,
                                                    verify, artwork, hash)
                                          sync/    (.itl reconcile, path_map,
                                                    conflict, worker)
                                          playback/(libmpv engine, prefetch,
                                                    ReplayGain, stats)
       ◄──── listen() ── events ───────── integration/ (MPRIS, tray, notify)
       (track-changed, position-update,
        sync-progress, organize-applied)
```

**Data model.** The schema lives in `src-tauri/prax/schema.prax` and is the single source
of truth: [Prax ORM](https://crates.io/crates/prax-orm) generates Rust types at compile
time, and `prax-typegen` generates the TypeScript + Zod models under
`src/app/models/generated`. It is deliberately denormalized to four tables — `Track`,
`Playlist`, `SyncSource`, `Preference`.

**Layout.**

| Path                        | Contents                                                                                                                   |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `src/app/components`        | 17 standalone Angular components (sidebar, track list, transport bar, album grid, smart-playlist editor, import wizard, …) |
| `src/app/services`          | Signal-based state + the `invoke` boundary                                                                                 |
| `src-tauri/src/commands`    | Tauri command surface (~46 commands)                                                                                       |
| `src-tauri/src/db`          | Query layer over Prax/SQLite                                                                                               |
| `src-tauri/src/fs`          | Ingest, organize, relink, verify, artwork, hashing                                                                         |
| `src-tauri/src/sync`        | iTunes `.itl` reconciliation pipeline                                                                                      |
| `src-tauri/src/playback`    | mpv engine, device config, play/skip stats                                                                                 |
| `src-tauri/src/integration` | MPRIS, tray, notifications                                                                                                 |
| `docs/plans`                | Design + implementation documents                                                                                          |

## Development

```bash
pnpm test              # Vitest, single run   (33 spec files, 342 cases)
pnpm run test:watch    # Vitest in watch mode
pnpm run test:coverage
pnpm run lint          # ESLint 10 + angular-eslint + unicorn
pnpm run format        # Prettier

cargo test   --manifest-path src-tauri/Cargo.toml --all              # 277 tests
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo fmt    --manifest-path src-tauri/Cargo.toml --all
```

CI runs exactly these on every push to `main`/`develop` and on every PR: codegen, lint,
format check, and Angular build for the frontend; `fmt`, `clippy -D warnings`, and
`cargo test --all` for the backend.

Commits follow [Conventional Commits](https://www.conventionalcommits.org/) — enforced by
commitlint, with lint-staged + Prettier wired through Husky.

Tests must never touch the desktop session; anything that would shell out to `xdg-open`
is gated behind `TUXTUNES_NO_XDG_OPEN`.

## Status

Pre-1.0 and under active development on the `develop` branch. The core loop — import,
browse, play, organize, sync — works; expect rough edges elsewhere.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.

### Third-party licenses

The dual license above covers TuxTunes' own source. Distributed binaries additionally
link against **libmpv**, which is **LGPL-2.1-or-later** (GPL-2.0-or-later if built with
its GPL-only components). TuxTunes links it dynamically against the system library, which
satisfies the LGPL, but anyone redistributing a build inherits that obligation — ship the
LGPL text and keep libmpv replaceable. Other dependencies carry their own terms; see
`Cargo.lock` and `pnpm-lock.yaml`.

## Credits

Built on [Tauri](https://tauri.app), [Angular](https://angular.dev),
[mpv](https://mpv.io), [Tailwind CSS](https://tailwindcss.com),
[Prax ORM](https://crates.io/crates/prax-orm), [itl-rs](https://crates.io/crates/itl-rs),
and [lofty](https://crates.io/crates/lofty).

Not affiliated with or endorsed by Apple Inc. iTunes is a trademark of Apple Inc.
