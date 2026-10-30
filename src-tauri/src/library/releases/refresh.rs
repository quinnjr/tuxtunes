//! One refresh of the feed: resolve artists, search release groups a
//! batch at a time, mark what the library already holds, and match
//! Qobuz albums. Pure over [`ReleaseSearch`] and [`AlbumLookup`] so it
//! runs in tests without a network.

use super::feed::{sort_newest_first, title_folds, ArtistRef, Feed, QobuzAlbum, Release};
use super::musicbrainz::{
    is_mbid, parse_page, release_group_query, ReleaseSearch, BATCH, MAX_PAGES, PAGE,
};
use super::qobuz::{AlbumLookup, AppIdRejected};
use crate::library::genres::musicbrainz::Rejected;
use crate::library::genres::resolve::fold_key;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};

/// How long an artist MusicBrainz could not match waits before it is
/// searched again.
pub const RETRY_UNMATCHED_DAYS: i64 = 30;

/// How long a matched artist's MBID is trusted before it is searched
/// again. MBIDs are stable, but MusicBrainz merges duplicate artists,
/// and the merged-away MBID then matches no releases at all.
pub const RECHECK_MATCHED_DAYS: i64 = 180;

/// How long a release Qobuz did not carry waits before it is searched
/// for again. New releases often reach Qobuz a few days late.
pub const QOBUZ_RETRY_DAYS: i64 = 7;

/// Failures in a row of one kind (artist searches, or batch searches)
/// that end a run.
/// One bad artist or batch is skipped and reported; a run of them means
/// MusicBrainz or the network is down, and pressing on would only burn
/// each request's retry budget in turn.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Refused artist searches in a row, with nothing yet answered this run,
/// that end it: MusicBrainz is refusing this client (a proxy, a block),
/// and the rest would be refused the same way. Higher than
/// [`MAX_CONSECUTIVE_FAILURES`] so that a few odd names at the top of a
/// library cannot stop every run.
pub const MAX_UNVOUCHED_REFUSALS: usize = 10;

/// A library artist, as `genres::resolve::artist_stats` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryArtist {
    /// Display spelling; the name searched for.
    pub key: String,
    /// `genres::resolve::fold_key` of the key; the cache key.
    pub fold: String,
    pub track_count: u64,
}

pub struct RefreshInput {
    pub artists: Vec<LibraryArtist>,
    /// Album titles in the library per artist fold, folded with
    /// `genres::musicbrainz::fold`.
    pub owned: HashMap<String, HashSet<String>>,
    /// MBIDs already known from `genre_map.json`, by artist fold.
    pub known_mbids: HashMap<String, String>,
    pub now: DateTime<Utc>,
    pub window_days: u32,
    /// `qobuz::app_id_key` of the app_id in use, when Qobuz is on.
    pub qobuz_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    /// Artists handled so far, of `total`.
    pub done: usize,
    pub total: usize,
    /// Releases in the feed as published at this checkpoint.
    pub releases: usize,
}

/// Run a refresh starting from `prev`. `checkpoint` is handed the feed
/// as it stands after every batch (and once at the end) so the caller
/// can save it and update the UI; the releases it carries are the ones
/// found so far plus `prev`'s for artists not reached yet, so the grid
/// never empties out mid-run. An error stops the run; whatever the last
/// checkpoint saw is kept.
pub async fn refresh<S: ReleaseSearch, Q: AlbumLookup>(
    mb: &S,
    qobuz: Option<&Q>,
    input: RefreshInput,
    prev: Feed,
    mut checkpoint: impl FnMut(&Feed, Progress),
) -> anyhow::Result<Feed> {
    let today = input.now.date_naive();
    let since = today - Duration::days(i64::from(input.window_days));
    let mut artists = input.artists;
    // Most-played first, so the artists that matter fill the grid first.
    artists.sort_by(|a, b| {
        b.track_count
            .cmp(&a.track_count)
            .then_with(|| a.key.cmp(&b.key))
    });
    let total = artists.len();

    // Misses found with another app_id (another storefront, or one that
    // did not work) say nothing about this one; only hits carry over.
    let same_app_id = input.qobuz_key.is_some() && input.qobuz_key == prev.qobuz_key;
    let prev_qobuz: HashMap<String, QobuzMatch> = prev
        .releases
        .iter()
        .map(|r| {
            let checked = r.qobuz_checked.filter(|_| r.qobuz.is_some() || same_app_id);
            (r.id.clone(), (r.qobuz.clone(), checked))
        })
        .collect();
    let mut run = Run {
        mb,
        qobuz,
        qobuz_error: None,
        qobuz_failures_in_row: 0,
        stale_misses: input.qobuz_key.is_some() && !same_app_id,
        prev_qobuz,
        owned: input.owned,
        since,
        today,
        found: Vec::new(),
        seen: HashSet::new(),
        reached: HashSet::new(),
        pending: Vec::new(),
        failed: HashSet::new(),
        retry_folds: HashMap::new(),
        failed_by_mbid: HashMap::new(),
        folds: HashMap::new(),
        flushed: HashMap::new(),
    };
    let mut feed = Feed {
        checked_at: prev.checked_at,
        // Until the run finishes, the stored releases are a mix of the
        // old window's and this run's, so they are only good for the
        // narrower of the two: an aborted widened run has not fetched the
        // older ones, and a narrowed run has already dropped them.
        window_days: prev.window_days.min(input.window_days),
        last_error: None,
        qobuz_error: None,
        // Set up front, not at the end: everything this run saves (an
        // aborted run's checkpoints included) has had misses from any
        // other app_id cleared, so the key always describes the misses.
        qobuz_key: input.qobuz_key.clone().or(prev.qobuz_key.clone()),
        ..prev.clone()
    };
    // MBID -> folds of every library artist that resolved to it. Two
    // spellings can fold apart ("Mötley Crüe", "Motley Crue") yet be
    // one MusicBrainz artist.
    let mut batch: Vec<(String, Vec<String>, String)> = Vec::new();
    let mut by_mbid: HashMap<String, usize> = HashMap::new();

    let mut failures = Failures::default();

    // Set by anything a checkpoint would save; the periodic checkpoint
    // is skipped when nothing has changed since the last.
    let mut dirty = false;
    // Artist searches MusicBrainz answered, and those it refused: if it
    // refused every one and no batch got through, it is
    // refusing this client (a proxy, a block), not those names.
    let mut answered = 0usize;
    // Refusals are held out of the cache (and so out of every saved
    // checkpoint) until MusicBrainz has vouched for itself this run; a run
    // cut short before then, by an abort, a quit or a crash, leaves no
    // misses behind for a client-wide block to have caused.
    let mut held: Vec<(String, String)> = Vec::new();
    let mut first_refusal: Option<String> = None;

    for (i, artist) in artists.iter().enumerate() {
        // Which artist a release belongs to is decided by this key on
        // both sides (here and `release_fold`), from the display spelling.
        let track = fold_key(&artist.key);
        let mbid = match resolve_mbid(mb, artist, &input.known_mbids, &mut feed, today).await {
            Ok((mbid, lookup)) => {
                match lookup {
                    // A cache hit says nothing about whether MusicBrainz is up.
                    Lookup::Cached => {}
                    Lookup::Adopted => dirty = true,
                    Lookup::Searched => {
                        failures.lookup_ok();
                        answered += 1;
                        commit_refusals(&mut feed, &mut held, today);
                        dirty = true;
                    }
                    Lookup::Refused(e) => {
                        first_refusal.get_or_insert_with(|| format!("{e:#}"));
                        held.push((artist.fold.clone(), artist.key.clone()));
                        // A paced request either way: report progress,
                        // though the miss itself stays held.
                        dirty = true;
                        if answered > 0 || failures.batches_ok > 0 {
                            commit_refusals(&mut feed, &mut held, today);
                        } else if held.len() >= MAX_UNVOUCHED_REFUSALS {
                            publish(&mut feed, &run, &prev, &mut checkpoint, i + 1, total, false);
                            anyhow::bail!(
                                "MusicBrainz refused the first {} artist searches; it may be blocking this connection ({})",
                                held.len(),
                                first_refusal.unwrap_or_default()
                            );
                        }
                    }
                    Lookup::FellBack(e) => {
                        dirty = true;
                        if failures.record(&artist.key, &e, Kind::Lookup) {
                            publish(&mut feed, &run, &prev, &mut checkpoint, i, total, false);
                            return Err(e.context(failures.abort_message()));
                        }
                    }
                }
                mbid
            }
            Err(e) => {
                // The artist keeps its releases from the previous feed.
                run.failed.insert(track.clone());
                dirty = true;
                if failures.record(&artist.key, &e, Kind::Lookup) {
                    publish(&mut feed, &run, &prev, &mut checkpoint, i, total, false);
                    return Err(e.context(failures.abort_message()));
                }
                continue;
            }
        };
        run.pending.push((track, mbid.clone()));
        if let Some(mbid) = mbid {
            if let Some(key) = run.flushed.get(&mbid) {
                // An earlier batch already searched this MBID under
                // another spelling; only its albums still count.
                run.folds
                    .entry(key.clone())
                    .or_default()
                    .push(artist.fold.clone());
            } else {
                match by_mbid.get(&mbid) {
                    Some(&slot) => batch[slot].1.push(artist.fold.clone()),
                    None => {
                        by_mbid.insert(mbid.clone(), batch.len());
                        // Spellings whose batch failed earlier ride along,
                        // so their owned albums still count.
                        let mut folds = run.retry_folds.remove(&mbid).unwrap_or_default();
                        folds.push(artist.fold.clone());
                        batch.push((mbid, folds, artist.key.clone()));
                    }
                }
            }
        }
        let full = batch.len() >= BATCH;
        if full {
            dirty = true;
            by_mbid.clear();
            if let Err(e) = run.flush(&mut batch).await {
                if failures.record("a batch of artists", &e, Kind::Batch) {
                    publish(&mut feed, &run, &prev, &mut checkpoint, i + 1, total, false);
                    return Err(e.context(failures.abort_message()));
                }
            } else {
                failures.batch_ok();
                commit_refusals(&mut feed, &mut held, today);
            }
        }
        // Also checkpoint every BATCH artists, so a run through mostly
        // unmatched artists still saves its lookups and reports progress.
        if dirty && (full || (i + 1) % BATCH == 0) {
            publish(&mut feed, &run, &prev, &mut checkpoint, i + 1, total, false);
            dirty = false;
        }
    }
    match run.flush(&mut batch).await {
        Ok(true) => {
            failures.batch_ok();
            commit_refusals(&mut feed, &mut held, today);
        }
        Ok(false) => {}
        Err(e) => {
            failures.record("a batch of artists", &e, Kind::Batch);
        }
    }
    // An outage is reported as one, ahead of the refusals it left
    // unvouched for.
    if let Some(e) = failures.nothing_searched() {
        // Every batch failed: this is not a check, whatever the count.
        publish(&mut feed, &run, &prev, &mut checkpoint, total, total, false);
        anyhow::bail!(e);
    }
    // Still held at the end: nothing vouched, so MusicBrainz refused this
    // client, not those names. The held misses are dropped, not cached.
    if !held.is_empty() {
        publish(&mut feed, &run, &prev, &mut checkpoint, total, total, false);
        anyhow::bail!(
            "MusicBrainz refused every artist search ({})",
            first_refusal.unwrap_or_default()
        );
    }
    run.remark_owned();
    feed.checked_at = Some(input.now);
    // Releases carried for failed artists come from the old window; only
    // when there are any is the feed narrower than this run's window.
    let carries_old = prev.checked_at.is_some()
        && prev
            .releases
            .iter()
            .any(|r| run.failed.contains(&release_fold(r)));
    feed.window_days = if carries_old {
        prev.window_days.min(input.window_days)
    } else {
        input.window_days
    };
    feed.last_error = failures.summary();
    // Forget artists no longer in the library, so the file does not
    // grow with every artist the library ever held.
    let present: HashSet<&str> = artists.iter().map(|a| a.fold.as_str()).collect();
    feed.artists
        .retain(|fold, _| present.contains(fold.as_str()));
    // Artists that failed keep their releases from the previous feed;
    // everyone else's (including artists gone from the library) are
    // replaced by what this run found.
    publish(&mut feed, &run, &prev, &mut checkpoint, total, total, true);
    Ok(feed)
}

#[derive(Clone, Copy)]
enum Kind {
    Lookup,
    Batch,
}

/// Failures skipped so far in a run. Lookups and batches are counted
/// apart: after the first run nearly every artist is a cache hit, so
/// lookups cannot vouch for MusicBrainz between two failing batches.
#[derive(Default)]
struct Failures {
    lookups_failed: usize,
    lookups_in_row: u32,
    batches_in_row: u32,
    batches_ok: usize,
    batches_failed: usize,
    first: Option<String>,
}

impl Failures {
    fn lookup_ok(&mut self) {
        self.lookups_in_row = 0;
    }

    fn batch_ok(&mut self) {
        // A search that got through vouches for MusicBrainz as much as a
        // lookup does.
        self.lookups_in_row = 0;
        self.batches_in_row = 0;
        self.batches_ok += 1;
    }

    /// Note a failure; true when the run should stop.
    fn record(&mut self, what: &str, e: &anyhow::Error, kind: Kind) -> bool {
        log::warn!("new releases: skipping {what}: {e:#}");
        self.first.get_or_insert_with(|| format!("{what}: {e:#}"));
        let in_row = match kind {
            Kind::Lookup => {
                self.lookups_failed += 1;
                &mut self.lookups_in_row
            }
            Kind::Batch => {
                self.batches_failed += 1;
                &mut self.batches_in_row
            }
        };
        *in_row += 1;
        *in_row >= MAX_CONSECUTIVE_FAILURES
    }

    /// The error to end the run with when batches were attempted and
    /// none succeeded.
    fn nothing_searched(&self) -> Option<String> {
        (self.batches_ok == 0 && self.batches_failed > 0).then(|| {
            format!(
                "no release search succeeded ({})",
                self.first.as_deref().unwrap_or("unknown error")
            )
        })
    }

    fn abort_message(&self) -> String {
        format!("stopped after {MAX_CONSECUTIVE_FAILURES} failures in a row")
    }

    /// For `Feed::last_error` on a run that finished despite failures.
    fn summary(&self) -> Option<String> {
        let first = self.first.as_ref()?;
        let count = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let parts: Vec<String> = [
            count(self.lookups_failed, "artist lookup", "artist lookups"),
            count(self.batches_failed, "release search", "release searches"),
        ]
        .into_iter()
        .flatten()
        .collect();
        // Not "skipped": a failed re-search falls back to the cached MBID.
        Some(format!("{} failed (first: {first})", parts.join(" and ")))
    }
}

/// The genre map's MBID when it has one (it may be a hand correction),
/// else the cached one, else a search (unless a recent search already
/// came up empty). Records the answer in `feed`; also says whether it
/// had to ask MusicBrainz. An MBID that is not a well-formed UUID (a
/// slip in a hand-edited file) is ignored rather than put in a query,
/// where it would break the search for its whole batch.
async fn resolve_mbid<S: ReleaseSearch>(
    mb: &S,
    artist: &LibraryArtist,
    known: &HashMap<String, String>,
    feed: &mut Feed,
    today: NaiveDate,
) -> anyhow::Result<(Option<String>, Lookup)> {
    // MBIDs are compared as strings against the lowercase ids in
    // MusicBrainz's replies, so a hand-pasted uppercase one is folded.
    let known = known
        .get(&artist.fold)
        .filter(|id| {
            let ok = is_mbid(id);
            if !ok {
                log::warn!(
                    "new releases: ignoring malformed MBID {id:?} for {:?}",
                    artist.key
                );
            }
            ok
        })
        .map(|id| id.to_ascii_lowercase());
    let cached = feed
        .artists
        .get(&artist.fold)
        .filter(|c| c.mbid.as_deref().is_none_or(is_mbid));
    let (mbid, lookup) = if let Some(id) = known {
        if cached.is_some_and(|c| c.mbid.as_deref() == Some(id.as_str())) {
            return Ok((Some(id), Lookup::Cached));
        }
        (Some(id), Lookup::Adopted)
    } else {
        // A matched artist due for its periodic re-search keeps its MBID
        // if that search fails: it is very likely still right.
        let mut stale_match = None;
        if let Some(cached) = cached {
            let ttl = if cached.mbid.is_some() {
                RECHECK_MATCHED_DAYS
            } else {
                RETRY_UNMATCHED_DAYS
            };
            let cached_mbid = cached.mbid.as_deref().map(str::to_ascii_lowercase);
            if today - cached.looked_up < Duration::days(ttl) {
                return Ok((cached_mbid, Lookup::Cached));
            }
            stale_match = cached_mbid;
        }
        let reply = mb.find_mbid(&artist.key).await;
        let reply = match (reply, &stale_match) {
            (Err(e), Some(id)) => {
                // Not recorded, so the re-search is tried again next run.
                return Ok((Some(id.clone()), Lookup::FellBack(e)));
            }
            (reply, _) => reply,
        };
        match reply {
            Ok(found) => {
                let found = found
                    .filter(|id| is_mbid(id))
                    .map(|id| id.to_ascii_lowercase());
                // No confident match any more (the artist was renamed, say)
                // does not unseat an MBID that still finds their releases.
                (found.or(stale_match), Lookup::Searched)
            }
            // A name MusicBrainz refuses outright gets the same answer
            // next time; cache it as a miss so it waits out the retry
            // interval instead of failing (and reporting) every run.
            Err(e) if e.downcast_ref::<Rejected>().is_some() => {
                log::warn!(
                    "new releases: treating {:?} as unmatched: {e:#}",
                    artist.key
                );
                // Not cached here: the run holds it until MusicBrainz
                // has vouched for itself (see `commit_refusals`).
                return Ok((None, Lookup::Refused(e)));
            }
            Err(e) => return Err(e),
        }
    };
    feed.artists.insert(
        artist.fold.clone(),
        ArtistRef {
            name: artist.key.clone(),
            mbid: mbid.clone(),
            looked_up: today,
        },
    );
    Ok((mbid, lookup))
}

/// Cache the held refusals as misses, now that MusicBrainz has shown
/// this run that it answers this client.
fn commit_refusals(feed: &mut Feed, held: &mut Vec<(String, String)>, today: NaiveDate) {
    for (fold, name) in held.drain(..) {
        feed.artists.insert(
            fold,
            ArtistRef {
                name,
                mbid: None,
                looked_up: today,
            },
        );
    }
}

/// How `resolve_mbid` came by its answer.
enum Lookup {
    /// From the cache; MusicBrainz was not asked.
    Cached,
    /// The genre map's MBID replaced the cached answer.
    Adopted,
    /// MusicBrainz was asked and answered.
    Searched,
    /// MusicBrainz refused the search. Not cached by `resolve_mbid`; the
    /// run caches it as a miss once MusicBrainz has vouched for itself,
    /// since a proxy refusing every request looks just like this.
    Refused(anyhow::Error),
    /// A due re-search failed; the cached MBID stands in. Counts as a
    /// failure, so an outage still ends the run, but the artist is not.
    FellBack(anyhow::Error),
}

/// The key a release's library artist is tracked under: `fold_key` of
/// the display spelling, as the run computes for each artist. (Not the
/// SQL fold in `LibraryArtist::fold`, whose `TRIM` only strips spaces.)
fn release_fold(r: &Release) -> String {
    fold_key(&r.artist_key)
}

/// Hand the checkpoint the releases found so far plus the previous
/// feed's for artists this run has not searched: mid-run, every artist
/// not reached yet; at the end (`last`), only the ones that failed.
#[allow(clippy::too_many_arguments)]
fn publish<S, Q>(
    feed: &mut Feed,
    run: &Run<'_, S, Q>,
    prev: &Feed,
    checkpoint: &mut impl FnMut(&Feed, Progress),
    done: usize,
    total: usize,
    last: bool,
) {
    let carried = |r: &&Release| {
        let unsearched = if last {
            run.failed.contains(&release_fold(r))
        } else {
            !run.reached.contains(&release_fold(r))
        };
        unsearched && !run.seen.contains(&r.id)
    };
    let mut releases = run.found.clone();
    releases.extend(prev.releases.iter().filter(carried).map(|r| {
        let mut r = r.clone();
        // A carried miss was found with the previous app_id; under a new
        // one it is unknown, so the next run asks again.
        if run.stale_misses && r.qobuz.is_none() {
            r.qobuz_checked = None;
        }
        r
    }));
    sort_newest_first(&mut releases);
    feed.releases = releases;
    feed.qobuz_error = run.qobuz_error.clone();
    checkpoint(
        feed,
        Progress {
            done,
            total,
            releases: feed.releases.len(),
        },
    );
}

/// A Qobuz album (or none) and the day Qobuz was last asked.
type QobuzMatch = (Option<QobuzAlbum>, Option<NaiveDate>);

struct Run<'a, S, Q> {
    mb: &'a S,
    qobuz: Option<&'a Q>,
    /// Set when Qobuz matching stops for the run.
    qobuz_error: Option<String>,
    qobuz_failures_in_row: u32,
    /// The app_id changed since the previous feed: its misses are void.
    stale_misses: bool,
    /// Last run's Qobuz answer per release-group id.
    prev_qobuz: HashMap<String, QobuzMatch>,
    owned: HashMap<String, HashSet<String>>,
    since: NaiveDate,
    today: NaiveDate,
    found: Vec<Release>,
    /// Release-group ids already in `found`.
    seen: HashSet<String>,
    // Artists are tracked by fold, not display key: the most common
    // spelling within a fold, and so a release's `artist_key`, can change
    // between runs.
    /// Artist folds whose releases this run has searched.
    reached: HashSet<String>,
    /// Artist folds resolved but not yet searched (their batch is open),
    /// with the MBID each resolved to.
    pending: Vec<(String, Option<String>)>,
    /// Artist folds whose lookup or batch search failed this run.
    failed: HashSet<String>,
    /// Folds of a failed batch's entries, by MBID, for the next spelling
    /// of the same artist to carry into its own batch.
    retry_folds: HashMap<String, Vec<String>>,
    /// Tracking keys of artists a failed batch left unsearched, by MBID,
    /// so a later batch that searches the MBID can settle them.
    failed_by_mbid: HashMap<String, Vec<String>>,
    /// Library artist folds per release `artist_key`, across batches.
    folds: HashMap<String, Vec<String>>,
    /// MBIDs already searched, to the artist key they were filed under.
    flushed: HashMap<String, String>,
}

impl<S: ReleaseSearch, Q: AlbumLookup> Run<'_, S, Q> {
    /// Search the batch's artists, page by page, and drain it. `Ok(true)`
    /// when it searched, `Ok(false)` when the batch was empty.
    async fn flush(
        &mut self,
        batch: &mut Vec<(String, Vec<String>, String)>,
    ) -> anyhow::Result<bool> {
        if batch.is_empty() {
            self.reached
                .extend(self.pending.drain(..).map(|(fold, _)| fold));
            return Ok(false);
        }
        let mbids: Vec<String> = batch.iter().map(|(id, _, _)| id.clone()).collect();
        let keys: HashMap<String, String> = batch
            .iter()
            .map(|(id, _, key)| (id.clone(), key.clone()))
            .collect();
        let entries = std::mem::take(batch);
        for (mbid, folds, key) in &entries {
            self.folds
                .entry(key.clone())
                .or_default()
                .extend(folds.iter().cloned());
            self.flushed.insert(mbid.clone(), key.clone());
        }
        let result = self.search(&mbids, &keys).await;
        if result.is_ok() {
            self.reached
                .extend(self.pending.drain(..).map(|(fold, _)| fold));
            // Spellings that failed in an earlier batch were searched now
            // under this MBID; they are no longer unsearched.
            for mbid in &mbids {
                for track in self.failed_by_mbid.remove(mbid).unwrap_or_default() {
                    self.failed.remove(&track);
                    self.reached.insert(track);
                }
            }
        } else {
            // Only artists in this batch went unsearched: those with no
            // MBID, or one an earlier batch searched, are settled. The
            // unsearched keep their previous releases, and a later
            // spelling of them searches again, with their folds.
            let failed_mbids: HashSet<&String> = mbids.iter().collect();
            for (fold, mbid) in self.pending.drain(..) {
                if let Some(m) = mbid.filter(|m| failed_mbids.contains(m)) {
                    self.failed_by_mbid.entry(m).or_default().push(fold.clone());
                    self.failed.insert(fold);
                } else {
                    self.reached.insert(fold);
                }
            }
            // `folds` keeps the entries: releases the first pages found
            // before the failure are still marked against them.
            for (mbid, folds, _) in entries {
                self.flushed.remove(&mbid);
                self.retry_folds.insert(mbid, folds);
            }
        }
        result.map(|()| true)
    }

    /// Read every page for `mbids` into `found`. A batch with more hits
    /// than [`MAX_PAGES`] pages hold is split in half and each half
    /// searched in turn, so nobody's releases are cut off for sharing a
    /// batch with a prolific artist; only a single artist past the cap
    /// loses the excess (with a warning).
    async fn search(
        &mut self,
        mbids: &[String],
        keys: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let cap = MAX_PAGES * PAGE;
        let mut groups = vec![mbids.to_vec()];
        while let Some(group) = groups.pop() {
            let query = release_group_query(&group, self.since, self.today);
            let mut offset = 0;
            loop {
                let page = parse_page(&self.mb.release_groups(&query, offset).await?, keys)?;
                let split = offset == 0 && page.count > cap && group.len() > 1;
                if split {
                    // The halves return these again; taking them here
                    // would only cost Qobuz lookups twice over.
                    let (a, b) = group.split_at(group.len() / 2);
                    groups.push(a.to_vec());
                    groups.push(b.to_vec());
                    break;
                }
                self.take(page.releases).await;
                offset += PAGE;
                if offset >= page.count {
                    break;
                }
                if offset >= cap {
                    log::warn!(
                        "new releases: {} has {} releases in the window; keeping the first {cap}",
                        group[0],
                        page.count
                    );
                    break;
                }
            }
        }
        Ok(())
    }

    /// Add newly seen releases to `found`, marked and matched.
    async fn take(&mut self, releases: Vec<Release>) {
        for mut release in releases {
            if !self.seen.insert(release.id.clone()) {
                continue;
            }
            release.in_library = self.owns(&release.artist_key, &release.title);
            (release.qobuz, release.qobuz_checked) = self.match_qobuz(&release).await;
            self.found.push(release);
        }
    }

    /// Whether any library album by the artist filed as `artist_key` is
    /// this title. Edition suffixes are dropped on both sides (see
    /// [`title_folds`]), so an owned "Album (Deluxe)" covers a new
    /// "Album", and an owned "Album" a new "Album (Deluxe Edition)".
    fn owns(&self, artist_key: &str, title: &str) -> bool {
        let wants: Vec<String> = title_folds(title).filter(|w| !w.is_empty()).collect();
        !wants.is_empty()
            && self.folds.get(artist_key).is_some_and(|folds| {
                folds.iter().any(|f| {
                    self.owned
                        .get(f)
                        .is_some_and(|albums| wants.iter().any(|w| albums.contains(w)))
                })
            })
    }

    /// Re-mark ownership once every spelling is known: a later batch
    /// can add a spelling whose albums an earlier batch's releases did
    /// not see.
    fn remark_owned(&mut self) {
        let marks: Vec<bool> = self
            .found
            .iter()
            .map(|r| self.owns(&r.artist_key, &r.title))
            .collect();
        for (r, owned) in self.found.iter_mut().zip(marks) {
            r.in_library = owned;
        }
    }

    /// Reuse last run's hit, and last run's miss while it is recent;
    /// otherwise ask Qobuz (unless it already failed this run).
    async fn match_qobuz(&mut self, release: &Release) -> QobuzMatch {
        let (prev_hit, prev_checked) = self
            .prev_qobuz
            .get(&release.id)
            .cloned()
            .unwrap_or_default();
        if prev_hit.is_some() {
            return (prev_hit, prev_checked);
        }
        let recent_miss =
            prev_checked.is_some_and(|d| self.today - d < Duration::days(QOBUZ_RETRY_DAYS));
        let Some(qobuz) = self
            .qobuz
            .filter(|_| self.qobuz_error.is_none() && !recent_miss)
        else {
            return (None, prev_checked);
        };
        // The artist the library knows, not the full credit: Qobuz
        // lists "A & B" releases under the main artist.
        match qobuz.find_album(&release.artist_key, &release.title).await {
            Ok(hit) => {
                self.qobuz_failures_in_row = 0;
                (hit, Some(self.today))
            }
            Err(e) => {
                // A refused app_id stops matching at once; a timeout or a
                // 5xx costs only this release, unless they keep coming.
                self.qobuz_failures_in_row += 1;
                let give_up = e.downcast_ref::<AppIdRejected>().is_some()
                    || self.qobuz_failures_in_row >= MAX_CONSECUTIVE_FAILURES;
                if give_up {
                    log::warn!("qobuz: giving up on matching for this run: {e:#}");
                    self.qobuz_error = Some(format!("{e:#}"));
                } else {
                    log::warn!("qobuz: skipping {:?}: {e:#}", release.title);
                }
                (None, prev_checked)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeMb {
        mbids: HashMap<String, String>,
        /// Release groups by artist MBID.
        groups: HashMap<String, Vec<Value>>,
        /// Artist names whose lookup fails.
        failing: HashSet<String>,
        /// Every release-group search fails.
        searches_fail: bool,
        /// Release-group searches naming this short artist id fail.
        search_fails_for: Option<String>,
        /// Artist names MusicBrainz refuses outright.
        rejected: HashSet<String>,
        searched: Mutex<Vec<String>>,
        queries: Mutex<Vec<(String, u32)>>,
    }

    impl ReleaseSearch for FakeMb {
        async fn find_mbid(&self, name: &str) -> anyhow::Result<Option<String>> {
            self.searched.lock().unwrap().push(name.to_string());
            if self.rejected.contains(name) {
                return Err(
                    Rejected(format!("musicbrainz rejected the request for {name:?}")).into(),
                );
            }
            if self.failing.contains(name) {
                anyhow::bail!("musicbrainz returned 400 for {name:?}");
            }
            Ok(self.mbids.get(name).map(|short| mbid(short)))
        }

        async fn release_groups(&self, query: &str, offset: u32) -> anyhow::Result<Value> {
            self.queries
                .lock()
                .unwrap()
                .push((query.to_string(), offset));
            if self.searches_fail
                || self
                    .search_fails_for
                    .as_ref()
                    .is_some_and(|short| query.contains(&mbid(short)))
            {
                anyhow::bail!("musicbrainz unavailable after 24 attempts");
            }
            let mut all: Vec<Value> = self
                .groups
                .iter()
                .filter(|(short, _)| query.contains(&format!("arid:{}", mbid(short))))
                .flat_map(|(_, g)| g.clone())
                .collect();
            all.sort_by_key(|g| g["id"].as_str().unwrap().to_string());
            let count = all.len();
            let page: Vec<Value> = all
                .into_iter()
                .skip(offset as usize)
                .take(PAGE as usize)
                .collect();
            Ok(json!({"count": count, "release-groups": page}))
        }
    }

    #[derive(Default)]
    struct FakeQobuz {
        fail: bool,
        /// Answer every lookup with "not on Qobuz".
        miss: bool,
        /// Lookups left to time out before answering normally.
        timeouts: Mutex<u32>,
        calls: Mutex<u32>,
    }

    impl AlbumLookup for FakeQobuz {
        async fn find_album(
            &self,
            artist: &str,
            title: &str,
        ) -> anyhow::Result<Option<QobuzAlbum>> {
            *self.calls.lock().unwrap() += 1;
            if self.fail {
                return Err(AppIdRejected("Qobuz rejected the app_id: nope".into()).into());
            }
            {
                let mut left = self.timeouts.lock().unwrap();
                if *left > 0 {
                    *left -= 1;
                    anyhow::bail!("error sending request: operation timed out");
                }
            }
            if self.miss {
                return Ok(None);
            }
            Ok(Some(QobuzAlbum {
                id: format!("{artist}/{title}"),
                image: None,
            }))
        }
    }

    /// A well-formed MBID standing for the short test id `short`, so the
    /// tests can name artists "a" or "id03" and still pass `is_mbid`.
    fn mbid(short: &str) -> String {
        let h = short.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        format!(
            "{:08x}-0000-4000-8000-{:012x}",
            h >> 32,
            h & 0xffff_ffff_ffff
        )
    }

    fn group(id: &str, title: &str, date: &str, short_mbid: &str, name: &str) -> Value {
        json!({
            "id": id, "title": title, "primary-type": "Album", "first-release-date": date,
            "artist-credit": [{"name": name, "artist": {"id": mbid(short_mbid)}}]
        })
    }

    fn artist(key: &str, tracks: u64) -> LibraryArtist {
        LibraryArtist {
            key: key.into(),
            fold: key.to_ascii_lowercase(),
            track_count: tracks,
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-10-30T12:00:00Z".parse().unwrap()
    }

    fn input(artists: Vec<LibraryArtist>) -> RefreshInput {
        RefreshInput {
            artists,
            owned: HashMap::new(),
            known_mbids: HashMap::new(),
            now: now(),
            window_days: 90,
            qobuz_key: None,
        }
    }

    const NO_QOBUZ: Option<&FakeQobuz> = None;

    #[tokio::test]
    async fn finds_releases_marks_owned_and_caches_mbids() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![
                    group("rg1", "First", "2026-09-01", "a", "Alpha"),
                    group("rg2", "Second", "2026-10-10", "a", "Alpha"),
                ],
            )]),
            ..FakeMb::default()
        };
        let mut inp = input(vec![artist("Alpha", 10), artist("Nobody", 1)]);
        inp.owned.insert(
            "alpha".into(),
            title_folds("First (Deluxe Edition)")
                .chain(title_folds("Secondhand"))
                .collect(),
        );
        let mut checkpoints = 0;
        let feed = refresh(&mb, NO_QOBUZ, inp, Feed::default(), |_, _| checkpoints += 1)
            .await
            .unwrap();
        assert_eq!(checkpoints, 1);
        let ids: Vec<&str> = feed.releases.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["rg2", "rg1"]);
        assert!(feed.releases[1].in_library);
        assert!(!feed.releases[0].in_library);
        assert_eq!(feed.checked_at, Some(now()));
        assert_eq!(feed.artists["alpha"].mbid, Some(mbid("a")));
        assert_eq!(feed.artists["nobody"].mbid, None);
        let query = mb.queries.lock().unwrap()[0].0.clone();
        assert!(query.contains("firstreleasedate:[2026-08 TO 2026-10-30]"));

        // A second run reuses both answers, the miss included.
        mb.searched.lock().unwrap().clear();
        let mut inp = input(vec![artist("Alpha", 10), artist("Nobody", 1)]);
        inp.now = now() + Duration::days(1);
        refresh(&mb, NO_QOBUZ, inp, feed, |_, _| {}).await.unwrap();
        assert!(mb.searched.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retries_unmatched_artists_after_a_month_and_uses_genre_map_mbids() {
        let mb = FakeMb {
            mbids: HashMap::from([("Late".into(), "l".into())]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        prev.artists.insert(
            "late".into(),
            ArtistRef {
                name: "Late".into(),
                mbid: None,
                looked_up: now().date_naive() - Duration::days(RETRY_UNMATCHED_DAYS),
            },
        );
        // The feed's own search once matched the wrong artist; the genre
        // map (perhaps hand-corrected) wins.
        prev.artists.insert(
            "mapped".into(),
            ArtistRef {
                name: "Mapped".into(),
                mbid: Some(mbid("wrong")),
                looked_up: now().date_naive(),
            },
        );
        let mut inp = input(vec![artist("Late", 1), artist("Mapped", 1)]);
        inp.known_mbids.insert("mapped".into(), mbid("m"));
        let feed = refresh(&mb, NO_QOBUZ, inp, prev, |_, _| {}).await.unwrap();
        assert_eq!(*mb.searched.lock().unwrap(), ["Late"]);
        assert_eq!(feed.artists["late"].mbid, Some(mbid("l")));
        assert_eq!(feed.artists["mapped"].mbid, Some(mbid("m")));
    }

    #[tokio::test]
    async fn batches_artists_pages_results_and_keeps_unreached_releases_visible() {
        let mut mb = FakeMb::default();
        let mut artists = Vec::new();
        for i in 0..(BATCH + 1) {
            let key = format!("Artist{i:02}");
            mb.mbids.insert(key.clone(), format!("id{i:02}"));
            // Fewer tracks for later artists so the sort keeps this order.
            artists.push(artist(&key, 100 - i as u64));
        }
        // The first artist has more than a page of releases.
        mb.groups.insert(
            "id00".into(),
            (0..(PAGE + 5))
                .map(|n| {
                    group(
                        &format!("rg{n:03}"),
                        &format!("T{n}"),
                        "2026-10-01",
                        "id00",
                        "Artist00",
                    )
                })
                .collect(),
        );
        let last = format!("id{BATCH:02}");
        mb.groups.insert(
            last.clone(),
            vec![group("rgLast", "Last", "2026-10-02", &last, "x")],
        );

        let mut prev = Feed::default();
        let mut stale = group_release("old", &format!("Artist{BATCH:02}"));
        stale.release_date = "2026-09-09".into();
        prev.releases.push(stale);

        let mut seen: Vec<(Progress, Vec<String>)> = Vec::new();
        let feed = refresh(&mb, NO_QOBUZ, input(artists), prev, |f, p| {
            seen.push((p, f.releases.iter().map(|r| r.id.clone()).collect()))
        })
        .await
        .unwrap();

        let queries = mb.queries.lock().unwrap();
        assert_eq!(
            queries.len(),
            3,
            "two pages for the first batch, one for the second"
        );
        assert_eq!(queries[1].1, PAGE);
        assert_eq!(seen.len(), 2);
        // Mid-run, the unreached artist's old release is still shown.
        assert_eq!(seen[0].0.done, BATCH);
        assert!(seen[0].1.contains(&"old".to_string()));
        // At the end it is replaced by what this run found.
        assert!(!seen[1].1.contains(&"old".to_string()));
        assert_eq!(feed.releases.len(), PAGE as usize + 6);
        assert_eq!(feed.releases[0].id, "rgLast");
    }

    fn group_release(id: &str, artist_key: &str) -> Release {
        Release {
            id: id.into(),
            title: id.into(),
            artist: artist_key.into(),
            artist_key: artist_key.into(),
            primary_type: None,
            secondary_types: vec![],
            release_date: "2026-10-01".into(),
            in_library: false,
            qobuz: None,
            qobuz_checked: None,
        }
    }

    #[tokio::test]
    async fn qobuz_matches_are_reused_and_a_failure_stops_matching() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![
                    group("rg1", "One", "2026-10-01", "a", "Alpha"),
                    group("rg2", "Two", "2026-10-02", "a", "Alpha"),
                    group("rg3", "Three", "2026-10-03", "a", "Alpha"),
                ],
            )]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        let mut known = group_release("rg1", "Alpha");
        known.qobuz = Some(QobuzAlbum {
            id: "kept".into(),
            image: None,
        });
        prev.releases.push(known);

        let ok = FakeQobuz::default();
        let feed = refresh(
            &mb,
            Some(&ok),
            input(vec![artist("Alpha", 1)]),
            prev.clone(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(*ok.calls.lock().unwrap(), 2);
        let by_id: HashMap<&str, &Release> =
            feed.releases.iter().map(|r| (r.id.as_str(), r)).collect();
        assert_eq!(by_id["rg1"].qobuz.as_ref().unwrap().id, "kept");
        assert_eq!(by_id["rg2"].qobuz.as_ref().unwrap().id, "Alpha/Two");
        assert_eq!(feed.qobuz_error, None);

        let bad = FakeQobuz {
            fail: true,
            ..FakeQobuz::default()
        };
        let feed = refresh(
            &mb,
            Some(&bad),
            input(vec![artist("Alpha", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(*bad.calls.lock().unwrap(), 1);
        assert!(feed.qobuz_error.unwrap().contains("app_id"));
        assert_eq!(feed.releases.len(), 3);
    }

    #[tokio::test]
    async fn a_qobuz_miss_waits_a_week_before_it_is_searched_again() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![
                    group("recent", "One", "2026-10-01", "a", "Alpha"),
                    group("stale", "Two", "2026-10-02", "a", "Alpha"),
                    group("new", "Three", "2026-10-03", "a", "Alpha"),
                ],
            )]),
            ..FakeMb::default()
        };
        let today = now().date_naive();
        let mut prev = Feed::default();
        for (id, ago) in [("recent", 2), ("stale", QOBUZ_RETRY_DAYS)] {
            let mut r = group_release(id, "Alpha");
            r.qobuz_checked = Some(today - Duration::days(ago));
            prev.releases.push(r);
        }
        prev.qobuz_key = Some("k1".into());
        let with_key = |key: &str| {
            let mut inp = input(vec![artist("Alpha", 1)]);
            inp.qobuz_key = Some(key.into());
            inp
        };
        let qobuz = FakeQobuz {
            miss: true,
            ..FakeQobuz::default()
        };
        // Another app_id: every miss is asked again.
        let other = FakeQobuz {
            miss: true,
            ..FakeQobuz::default()
        };
        let switched = refresh(&mb, Some(&other), with_key("k2"), prev.clone(), |_, _| {})
            .await
            .unwrap();
        assert_eq!(*other.calls.lock().unwrap(), 3);
        assert_eq!(switched.qobuz_key.as_deref(), Some("k2"));

        let feed = refresh(&mb, Some(&qobuz), with_key("k1"), prev, |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            *qobuz.calls.lock().unwrap(),
            2,
            "only the stale and the new release"
        );
        let checked: HashMap<&str, Option<NaiveDate>> = feed
            .releases
            .iter()
            .map(|r| (r.id.as_str(), r.qobuz_checked))
            .collect();
        assert_eq!(checked["recent"], Some(today - Duration::days(2)));
        assert_eq!(checked["stale"], Some(today));
        assert_eq!(checked["new"], Some(today));
    }

    #[tokio::test]
    async fn a_failing_artist_is_skipped_and_keeps_its_old_releases() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![group("fresh", "Fresh", "2026-10-01", "a", "Alpha")],
            )]),
            failing: HashSet::from(["Broken".to_string()]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        prev.releases.push(group_release("kept", "Broken"));
        prev.releases
            .push(group_release("gone", "Removed From Library"));
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Alpha", 3), artist("Broken", 2)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        let mut ids: Vec<&str> = feed.releases.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["fresh", "kept"]);
        assert_eq!(feed.checked_at, Some(now()));
        let err = feed.last_error.unwrap();
        assert!(
            err.starts_with("1 artist lookup failed (first: Broken"),
            "{err}"
        );
        assert!(
            !feed.artists.contains_key("broken"),
            "a failure is not cached"
        );
    }

    #[tokio::test]
    async fn failures_in_a_row_stop_the_run_after_saving_progress() {
        let failing: HashSet<String> = (0..MAX_CONSECUTIVE_FAILURES)
            .map(|i| format!("Broken{i}"))
            .collect();
        let mut artists = vec![artist("Alpha", 100)];
        artists.extend(failing.iter().map(|k| artist(k, 1)));
        artists.push(artist("Never", 0));
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            failing,
            ..FakeMb::default()
        };
        let mut saved = Vec::new();
        let err = refresh(&mb, NO_QOBUZ, input(artists), Feed::default(), |f, _| {
            saved.push(f.clone())
        })
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("failures in a row"), "{err:#}");
        assert!(!mb.searched.lock().unwrap().contains(&"Never".to_string()));
        assert_eq!(saved.last().unwrap().artists["alpha"].mbid, Some(mbid("a")));
    }

    /// A feed whose every artist already has a cached MBID, as on any
    /// run after the first.
    fn cached_feed(keys: &[String]) -> Feed {
        let mut feed = Feed::default();
        for k in keys {
            feed.artists.insert(
                k.to_ascii_lowercase(),
                ArtistRef {
                    name: k.clone(),
                    mbid: Some(mbid(k)),
                    looked_up: now().date_naive(),
                },
            );
        }
        feed
    }

    #[tokio::test]
    async fn cache_hits_do_not_hide_an_outage_from_the_batch_count() {
        let keys: Vec<String> = (0..BATCH * MAX_CONSECUTIVE_FAILURES as usize + 5)
            .map(|i| format!("Artist{i:03}"))
            .collect();
        let mb = FakeMb {
            searches_fail: true,
            ..FakeMb::default()
        };
        let artists = keys.iter().map(|k| artist(k, 1)).collect();
        let err = refresh(&mb, NO_QOBUZ, input(artists), cached_feed(&keys), |_, _| {})
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failures in a row"), "{err:#}");
        assert_eq!(
            mb.queries.lock().unwrap().len(),
            MAX_CONSECUTIVE_FAILURES as usize
        );
    }

    #[tokio::test]
    async fn a_run_whose_only_batch_failed_is_not_a_check() {
        let keys = vec!["Alpha".to_string()];
        let mb = FakeMb {
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut saved = Vec::new();
        let err = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Alpha", 1)]),
            cached_feed(&keys),
            |f, _| saved.push(f.clone()),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no release search succeeded"),
            "{err:#}"
        );
        assert!(saved.iter().all(|f| f.checked_at.is_none()));
    }

    #[tokio::test]
    async fn the_window_is_recorded_only_when_a_run_finishes() {
        let keys = vec!["Alpha".to_string()];
        let mut prev = cached_feed(&keys);
        prev.window_days = 90;
        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.window_days = 365;
        let failing = FakeMb {
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut saved = Vec::new();
        let _ = refresh(&failing, NO_QOBUZ, inp, prev.clone(), |f, _| {
            saved.push(f.window_days)
        })
        .await;
        assert!(saved.iter().all(|&w| w == 90), "{saved:?}");

        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.window_days = 365;
        let feed = refresh(&FakeMb::default(), NO_QOBUZ, inp, prev, |_, _| {})
            .await
            .unwrap();
        assert_eq!(feed.window_days, 365);
    }

    #[tokio::test]
    async fn forgets_artists_gone_from_the_library_and_malformed_mbids() {
        let mut prev = cached_feed(&["Gone".to_string()]);
        prev.artists.insert(
            "typo".into(),
            ArtistRef {
                name: "Typo".into(),
                mbid: Some("not-a-uuid)".into()),
                looked_up: now().date_naive(),
            },
        );
        let mb = FakeMb {
            mbids: HashMap::from([("Typo".into(), "t".into())]),
            ..FakeMb::default()
        };
        let mut inp = input(vec![artist("Typo", 1), artist("Hand", 1)]);
        inp.known_mbids.insert("hand".into(), "pasted (url)".into());
        let feed = refresh(&mb, NO_QOBUZ, inp, prev, |_, _| {}).await.unwrap();
        assert!(!feed.artists.contains_key("gone"));
        // The malformed cached MBID was searched afresh.
        assert_eq!(feed.artists["typo"].mbid, Some(mbid("t")));
        // The malformed genre-map MBID fell back to a search, which missed.
        assert_eq!(*mb.searched.lock().unwrap(), ["Hand", "Typo"]);
        assert_eq!(feed.artists["hand"].mbid, None);
        let (query, _) = mb.queries.lock().unwrap()[0].clone();
        assert!(!query.contains("not-a-uuid") && !query.contains("pasted"));
    }

    #[tokio::test]
    async fn an_owned_album_covers_its_new_deluxe_edition() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![group(
                    "dlx",
                    "Album (Deluxe Edition)",
                    "2026-10-01",
                    "a",
                    "Alpha",
                )],
            )]),
            ..FakeMb::default()
        };
        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.owned
            .insert("alpha".into(), title_folds("Album").collect());
        let feed = refresh(&mb, NO_QOBUZ, inp, Feed::default(), |_, _| {})
            .await
            .unwrap();
        assert!(feed.releases[0].in_library);
    }

    #[tokio::test]
    async fn a_name_musicbrainz_refuses_is_cached_as_a_miss() {
        let mb = FakeMb {
            rejected: HashSet::from(["Very Long Credit".to_string()]),
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            ..FakeMb::default()
        };
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Alpha", 2), artist("Very Long Credit", 1)]),
            Feed::default(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(feed.last_error, None);
        let cached = &feed.artists["very long credit"];
        assert_eq!(cached.mbid, None);
        assert_eq!(cached.looked_up, now().date_naive());
    }

    #[tokio::test]
    async fn a_qobuz_timeout_costs_only_its_release() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![
                    group("rg1", "One", "2026-10-01", "a", "Alpha"),
                    group("rg2", "Two", "2026-10-02", "a", "Alpha"),
                ],
            )]),
            ..FakeMb::default()
        };
        let qobuz = FakeQobuz {
            timeouts: Mutex::new(1),
            ..FakeQobuz::default()
        };
        let feed = refresh(
            &mb,
            Some(&qobuz),
            input(vec![artist("Alpha", 1)]),
            Feed::default(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(feed.qobuz_error, None);
        let matched: Vec<bool> = feed.releases.iter().map(|r| r.qobuz.is_some()).collect();
        assert_eq!(matched.iter().filter(|m| **m).count(), 1);
        // The timed-out release was not marked as checked, so it is
        // asked again next run rather than waiting out a miss.
        assert!(feed
            .releases
            .iter()
            .any(|r| r.qobuz.is_none() && r.qobuz_checked.is_none()));
    }

    #[tokio::test]
    async fn a_crowded_batch_is_split_instead_of_cut_off() {
        let cap = (MAX_PAGES * PAGE) as usize;
        let mb = FakeMb {
            mbids: HashMap::from([("Busy".into(), "b".into()), ("Quiet".into(), "q".into())]),
            groups: HashMap::from([
                (
                    "b".into(),
                    (0..cap + 1)
                        .map(|n| {
                            group(
                                &format!("b{n:04}"),
                                &format!("T{n}"),
                                "2026-10-01",
                                "b",
                                "Busy",
                            )
                        })
                        .collect(),
                ),
                (
                    "q".into(),
                    vec![group("zz-quiet", "Quiet One", "2026-10-02", "q", "Quiet")],
                ),
            ]),
            ..FakeMb::default()
        };
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Busy", 2), artist("Quiet", 1)]),
            Feed::default(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(feed.releases.iter().any(|r| r.id == "zz-quiet"));
        // The lone busy artist keeps the first `cap` and drops the rest.
        assert_eq!(feed.releases.len(), cap + 1);
    }

    #[tokio::test]
    async fn the_summary_names_a_failed_release_search() {
        let mut mb = FakeMb {
            search_fails_for: Some(format!("id{BATCH:02}")),
            ..FakeMb::default()
        };
        let mut artists = Vec::new();
        for i in 0..=BATCH {
            let key = format!("Artist{i:02}");
            mb.mbids.insert(key.clone(), format!("id{i:02}"));
            artists.push(artist(&key, 100 - i as u64));
        }
        let feed = refresh(&mb, NO_QOBUZ, input(artists), Feed::default(), |_, _| {})
            .await
            .unwrap();
        let err = feed.last_error.unwrap();
        assert!(err.starts_with("1 release search failed (first:"), "{err}");
    }

    #[tokio::test]
    async fn a_narrowed_run_records_the_narrower_window_as_it_goes() {
        let keys = vec!["Alpha".to_string()];
        let mut prev = cached_feed(&keys);
        prev.window_days = 365;
        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.window_days = 30;
        let failing = FakeMb {
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut saved = Vec::new();
        let _ = refresh(&failing, NO_QOBUZ, inp, prev, |f, _| {
            saved.push(f.window_days)
        })
        .await;
        assert!(
            !saved.is_empty() && saved.iter().all(|&w| w == 30),
            "{saved:?}"
        );
    }

    #[tokio::test]
    async fn a_matched_artist_is_searched_again_after_half_a_year_and_mbids_are_lowercased() {
        let mb = FakeMb {
            mbids: HashMap::from([("Merged".into(), "merged-into".into())]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        prev.artists.insert(
            "merged".into(),
            ArtistRef {
                name: "Merged".into(),
                mbid: Some(mbid("merged-away")),
                looked_up: now().date_naive() - Duration::days(RECHECK_MATCHED_DAYS),
            },
        );
        let mut inp = input(vec![artist("Merged", 1), artist("Pasted", 1)]);
        inp.known_mbids
            .insert("pasted".into(), mbid("p").to_ascii_uppercase());
        let feed = refresh(&mb, NO_QOBUZ, inp, prev, |_, _| {}).await.unwrap();
        assert_eq!(feed.artists["merged"].mbid, Some(mbid("merged-into")));
        assert_eq!(feed.artists["pasted"].mbid, Some(mbid("p")));
    }

    #[tokio::test]
    async fn a_remix_ep_of_an_owned_album_is_new() {
        let mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            groups: HashMap::from([(
                "a".into(),
                vec![group(
                    "rmx",
                    "Blackstar (Remixes)",
                    "2026-10-01",
                    "a",
                    "Alpha",
                )],
            )]),
            ..FakeMb::default()
        };
        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.owned
            .insert("alpha".into(), title_folds("Blackstar").collect());
        let feed = refresh(&mb, NO_QOBUZ, inp, Feed::default(), |_, _| {})
            .await
            .unwrap();
        assert!(!feed.releases[0].in_library);
    }

    #[tokio::test]
    async fn misses_carried_through_a_failed_batch_are_voided_by_a_new_app_id() {
        let keys = vec!["Alpha".to_string()];
        let mut prev = cached_feed(&keys);
        prev.qobuz_key = Some("old".into());
        let mut carried = group_release("carried", "Alpha");
        carried.qobuz_checked = Some(now().date_naive());
        prev.releases.push(carried);
        let mb = FakeMb {
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut inp = input(vec![artist("Alpha", 1)]);
        inp.qobuz_key = Some("new".into());
        let qobuz = FakeQobuz::default();
        let mut saved = Vec::new();
        let _ = refresh(&mb, Some(&qobuz), inp, prev, |f, _| saved.push(f.clone())).await;
        let last = saved.last().unwrap();
        assert_eq!(last.qobuz_key.as_deref(), Some("new"));
        let r = last.releases.iter().find(|r| r.id == "carried").unwrap();
        assert_eq!(r.qobuz_checked, None);
    }

    #[tokio::test]
    async fn a_failed_recheck_keeps_the_cached_mbid() {
        let mb = FakeMb {
            failing: HashSet::from(["Alpha".to_string()]),
            groups: HashMap::from([(
                "a".into(),
                vec![group("rg", "New", "2026-10-01", "a", "Alpha")],
            )]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        let due = now().date_naive() - Duration::days(RECHECK_MATCHED_DAYS);
        prev.artists.insert(
            "alpha".into(),
            ArtistRef {
                name: "Alpha".into(),
                mbid: Some(mbid("a")),
                looked_up: due,
            },
        );
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Alpha", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(feed.releases.len(), 1);
        // Reported, though the artist was not skipped.
        assert!(feed.last_error.unwrap().contains("Alpha"));
        // Not stamped, so the re-search is tried again next run.
        assert_eq!(feed.artists["alpha"].looked_up, due);
    }

    #[tokio::test]
    async fn a_failed_batch_fails_only_its_own_artists() {
        // Batch one is Alpha plus 24 fillers and fails; "Unmatched"
        // resolved to nothing while that batch was open. Batch two (the
        // last filler and Beta) succeeds.
        let mut mb = FakeMb {
            mbids: HashMap::from([("Alpha".into(), "a".into()), ("Beta".into(), "b".into())]),
            search_fails_for: Some("a".into()),
            ..FakeMb::default()
        };
        let mut artists = vec![artist("Alpha", 100), artist("Unmatched", 99)];
        for i in 0..BATCH {
            let key = format!("Filler{i:02}");
            mb.mbids.insert(key.clone(), format!("f{i:02}"));
            artists.push(artist(&key, 50));
        }
        artists.push(artist("Beta", 1));
        let mut prev = Feed::default();
        prev.releases.push(group_release("old-alpha", "Alpha"));
        prev.releases
            .push(group_release("old-unmatched", "Unmatched"));
        let feed = refresh(&mb, NO_QOBUZ, input(artists), prev, |_, _| {})
            .await
            .unwrap();
        let ids: HashSet<&str> = feed.releases.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains("old-alpha"),
            "the failed artist keeps its releases"
        );
        assert!(!ids.contains("old-unmatched"), "a settled artist does not");
    }

    #[tokio::test]
    async fn carried_releases_follow_the_fold_not_the_spelling() {
        let mb = FakeMb {
            failing: HashSet::from(["bjork".to_string()]),
            mbids: HashMap::from([("Other".into(), "o".into())]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        prev.releases.push(group_release("old", "Bjork"));
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("bjork", 2), artist("Other", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(feed.releases.iter().any(|r| r.id == "old"));
    }

    #[tokio::test]
    async fn an_outage_during_rechecks_still_stops_the_run() {
        let keys: Vec<String> = (0..10).map(|i| format!("Artist{i}")).collect();
        let mut prev = cached_feed(&keys);
        for a in prev.artists.values_mut() {
            a.looked_up = now().date_naive() - Duration::days(RECHECK_MATCHED_DAYS);
        }
        let mb = FakeMb {
            failing: keys.iter().cloned().collect(),
            ..FakeMb::default()
        };
        let artists = keys.iter().map(|k| artist(k, 1)).collect();
        let err = refresh(&mb, NO_QOBUZ, input(artists), prev, |_, _| {})
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failures in a row"), "{err:#}");
        assert_eq!(
            mb.searched.lock().unwrap().len(),
            MAX_CONSECUTIVE_FAILURES as usize
        );
    }

    #[tokio::test]
    async fn a_spelling_searched_later_settles_its_failed_twin() {
        // "Mötley" (first batch, fails) and "Motley" (second batch, works)
        // share one MBID; the old release filed under the first spelling
        // is not carried once the second has searched that MBID.
        let mut mb = FakeMb {
            search_fails_for: Some("f00".into()),
            ..FakeMb::default()
        };
        mb.mbids.insert("Mötley".into(), "m".into());
        mb.mbids.insert("Motley".into(), "m".into());
        let mut artists = vec![artist("Mötley", 100)];
        for i in 0..BATCH - 1 {
            let key = format!("Filler{i:02}");
            mb.mbids.insert(key.clone(), format!("f{i:02}"));
            artists.push(artist(&key, 50));
        }
        artists.push(artist("Motley", 1));
        let mut prev = Feed::default();
        prev.releases.push(group_release("gone-from-mb", "Mötley"));
        let feed = refresh(&mb, NO_QOBUZ, input(artists), prev, |_, _| {})
            .await
            .unwrap();
        assert!(!feed.releases.iter().any(|r| r.id == "gone-from-mb"));
    }

    #[tokio::test]
    async fn odd_whitespace_in_a_tag_does_not_lose_a_failed_artists_releases() {
        let spelled = "Artist\u{a0}";
        let mb = FakeMb {
            failing: HashSet::from([spelled.to_string()]),
            mbids: HashMap::from([("Other".into(), "o".into())]),
            ..FakeMb::default()
        };
        let mut prev = Feed::default();
        prev.releases.push(group_release("old", spelled));
        // The SQL fold keeps the NBSP; the run must not compare it with
        // a Rust-trimmed fold of the release's spelling.
        let odd = LibraryArtist {
            key: spelled.into(),
            fold: "artist\u{a0}".into(),
            track_count: 2,
        };
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![odd, artist("Other", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(feed.releases.iter().any(|r| r.id == "old"));
    }

    #[tokio::test]
    async fn a_widened_run_with_failures_keeps_the_narrower_window() {
        let mb = FakeMb {
            failing: HashSet::from(["Broken".to_string()]),
            mbids: HashMap::from([("Alpha".into(), "a".into())]),
            ..FakeMb::default()
        };
        let widened = || {
            let mut inp = input(vec![artist("Alpha", 2), artist("Broken", 1)]);
            inp.window_days = 365;
            inp
        };
        // A failed artist with old-window releases narrows the feed.
        let mut prev = Feed {
            window_days: 90,
            checked_at: Some(now() - Duration::days(1)),
            ..Feed::default()
        };
        prev.releases.push(group_release("old", "Broken"));
        let feed = refresh(&mb, NO_QOBUZ, widened(), prev, |_, _| {})
            .await
            .unwrap();
        assert_eq!(feed.window_days, 90);

        // A first run (or a failure carrying nothing) does not.
        let feed = refresh(&mb, NO_QOBUZ, widened(), Feed::default(), |_, _| {})
            .await
            .unwrap();
        assert_eq!(feed.window_days, 365);
    }

    #[tokio::test]
    async fn quiet_stretches_are_not_checkpointed() {
        let keys: Vec<String> = (0..BATCH * 3).map(|i| format!("Quiet{i:03}")).collect();
        let mut prev = Feed::default();
        for k in &keys {
            prev.artists.insert(
                k.to_ascii_lowercase(),
                ArtistRef {
                    name: k.clone(),
                    mbid: None,
                    looked_up: now().date_naive(),
                },
            );
        }
        let mut checkpoints = 0;
        let artists = keys.iter().map(|k| artist(k, 1)).collect();
        refresh(
            &FakeMb::default(),
            NO_QOBUZ,
            input(artists),
            prev,
            |_, _| checkpoints += 1,
        )
        .await
        .unwrap();
        assert_eq!(checkpoints, 1, "only the final one");
    }

    #[tokio::test]
    async fn a_recheck_without_a_match_keeps_the_old_mbid() {
        let mb = FakeMb::default();
        let mut prev = Feed::default();
        prev.artists.insert(
            "renamed".into(),
            ArtistRef {
                name: "Renamed".into(),
                mbid: Some(mbid("r")),
                looked_up: now().date_naive() - Duration::days(RECHECK_MATCHED_DAYS),
            },
        );
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Renamed", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(feed.artists["renamed"].mbid, Some(mbid("r")));
        assert_eq!(feed.artists["renamed"].looked_up, now().date_naive());
    }

    #[tokio::test]
    async fn a_client_refused_everywhere_is_an_error_not_an_empty_feed() {
        let names = ["A", "B", "C"];
        let mb = FakeMb {
            rejected: names.iter().map(|s| s.to_string()).collect(),
            ..FakeMb::default()
        };
        let artists = names.iter().map(|k| artist(k, 1)).collect();
        let mut saved = Vec::new();
        let err = refresh(&mb, NO_QOBUZ, input(artists), Feed::default(), |f, _| {
            saved.push(f.clone())
        })
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused every artist search"),
            "{err:#}"
        );
        assert!(saved.last().unwrap().artists.is_empty(), "no misses kept");
    }

    #[tokio::test]
    async fn a_good_batch_breaks_a_lookup_failure_streak() {
        // Three always-failing artists, each followed by enough matched
        // artists to fill a batch that succeeds; cached, so no lookups.
        let mut mb = FakeMb::default();
        let mut artists = Vec::new();
        let mut keys = Vec::new();
        for b in 0..MAX_CONSECUTIVE_FAILURES {
            let broken = format!("Broken{b}");
            mb.failing.insert(broken.clone());
            artists.push(artist(&broken, 1000 - u64::from(b) * 100));
            for i in 0..BATCH {
                let key = format!("Fine{b}-{i:02}");
                keys.push(key.clone());
                artists.push(artist(&key, 999 - u64::from(b) * 100));
            }
        }
        let feed = refresh(&mb, NO_QOBUZ, input(artists), cached_feed(&keys), |_, _| {})
            .await
            .unwrap();
        assert!(feed
            .last_error
            .unwrap()
            .starts_with("3 artist lookups failed"));
    }

    #[tokio::test]
    async fn an_early_abort_while_refused_everywhere_keeps_no_misses() {
        // A cached library (no lookups needed) except a few artists due a
        // retry, all refused; every batch then fails and the run aborts.
        let fine: Vec<String> = (0..BATCH * MAX_CONSECUTIVE_FAILURES as usize)
            .map(|i| format!("Cached{i:03}"))
            .collect();
        let refused_names = ["R1", "R2"];
        let mut prev = cached_feed(&fine);
        for name in refused_names {
            prev.artists.insert(
                name.to_ascii_lowercase(),
                ArtistRef {
                    name: name.into(),
                    mbid: None,
                    looked_up: now().date_naive() - Duration::days(RETRY_UNMATCHED_DAYS),
                },
            );
        }
        let mb = FakeMb {
            rejected: refused_names.iter().map(|s| s.to_string()).collect(),
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut artists: Vec<LibraryArtist> =
            refused_names.iter().map(|k| artist(k, 1000)).collect();
        artists.extend(fine.iter().map(|k| artist(k, 1)));
        let mut saved = Vec::new();
        let err = refresh(&mb, NO_QOBUZ, input(artists), prev.clone(), |f, _| {
            saved.push(f.clone())
        })
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("failures in a row"), "{err:#}");
        let last = saved.last().unwrap();
        for name in refused_names {
            let fold = name.to_ascii_lowercase();
            assert_eq!(last.artists[&fold], prev.artists[&fold], "{name} restored");
        }
    }

    #[tokio::test]
    async fn no_checkpoint_saves_a_refusal_nothing_has_vouched_for() {
        // Refused everywhere, with enough artists for periodic
        // checkpoints: a quit or crash after any of them must find no
        // fresh misses on disk.
        let names: Vec<String> = (0..BATCH * 2 + 3).map(|i| format!("R{i:03}")).collect();
        let mb = FakeMb {
            rejected: names.iter().cloned().collect(),
            ..FakeMb::default()
        };
        let mut saved = Vec::new();
        let artists = names.iter().map(|k| artist(k, 1)).collect();
        let _ = refresh(&mb, NO_QOBUZ, input(artists), Feed::default(), |f, _| {
            saved.push(f.clone())
        })
        .await;
        assert!(!saved.is_empty());
        assert!(saved.iter().all(|f| f.artists.is_empty()));
    }

    #[tokio::test]
    async fn refusals_are_cached_once_musicbrainz_has_answered() {
        // A refused name before any answer is held, then cached once a
        // later lookup is answered.
        let mb = FakeMb {
            rejected: HashSet::from(["Refused".to_string()]),
            mbids: HashMap::from([("Later".into(), "l".into())]),
            ..FakeMb::default()
        };
        let feed = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Refused", 2), artist("Later", 1)]),
            Feed::default(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(feed.artists["refused"].mbid, None);
        assert_eq!(feed.artists["later"].mbid, Some(mbid("l")));
    }

    #[tokio::test]
    async fn a_client_refused_from_the_start_stops_early_and_reports_progress() {
        let names: Vec<String> = (0..MAX_UNVOUCHED_REFUSALS * 5)
            .map(|i| format!("R{i:03}"))
            .collect();
        let mb = FakeMb {
            rejected: names.iter().cloned().collect(),
            ..FakeMb::default()
        };
        let mut progress = Vec::new();
        let artists = names.iter().map(|k| artist(k, 1)).collect();
        let err = refresh(&mb, NO_QOBUZ, input(artists), Feed::default(), |f, p| {
            assert!(f.artists.is_empty());
            progress.push(p.done)
        })
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("may be blocking"), "{err:#}");
        assert_eq!(mb.searched.lock().unwrap().len(), MAX_UNVOUCHED_REFUSALS);
        assert_eq!(progress.last(), Some(&MAX_UNVOUCHED_REFUSALS));
    }

    #[tokio::test]
    async fn an_outage_is_reported_ahead_of_unvouched_refusals() {
        let keys = vec!["Cached".to_string()];
        let mb = FakeMb {
            rejected: HashSet::from(["Odd".to_string()]),
            searches_fail: true,
            ..FakeMb::default()
        };
        let mut prev = cached_feed(&keys);
        prev.artists.insert(
            "odd".into(),
            ArtistRef {
                name: "Odd".into(),
                mbid: None,
                looked_up: now().date_naive() - Duration::days(RETRY_UNMATCHED_DAYS),
            },
        );
        let err = refresh(
            &mb,
            NO_QOBUZ,
            input(vec![artist("Cached", 2), artist("Odd", 1)]),
            prev,
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no release search succeeded"),
            "{err:#}"
        );
    }
}
