//! Genre resolution and the generated per-artist playlist tree.
//!
//! `resolve` asks MusicBrainz what each artist plays and records it in
//! a map file; `apply` writes those genres onto tracks and files;
//! `rebuild` regenerates the sidebar as one folder per umbrella genre
//! with one smart playlist per artist. See
//! `docs/superpowers/specs/2026-09-14-genre-playlists-design.md`.

pub mod map;
pub mod musicbrainz;
pub mod taxonomy;
