//! "New From Your Artists": recent releases by the artists in the
//! library, shown as a cover grid that links out to Qobuz.
//!
//! `refresh` resolves each library artist to a MusicBrainz ID (cached
//! in the feed file, seeded from `genre_map.json` when genres have been
//! resolved), then asks MusicBrainz for release groups first released
//! inside the window, a batch of artists per request. When a Qobuz
//! app_id is configured each new release is matched to its Qobuz album
//! for an exact link and cover; without one the card links to a Qobuz
//! search and takes its cover from the Cover Art Archive.
//!
//! The feed is a cache file beside the database, not a table: it is
//! rebuilt from the network on every refresh and losing it costs only
//! the MBID lookups. `service` owns the background run.

pub mod feed;
pub mod musicbrainz;
pub mod qobuz;
pub mod refresh;
pub mod service;

pub use feed::{Feed, Release, ReleasePrefs};
