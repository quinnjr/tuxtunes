import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { InViewDirective } from '../../directives/in-view.directive';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { TauriService } from '../../services/tauri.service';
import { PlaylistAlbumSort, UiService } from '../../services/ui.service';
import { formatTotalDuration } from '../../utils/format';
import { formatMmSs } from '../../utils/time';

/** One album's slice of the open playlist. */
export interface PlaylistAlbum {
  /** Stable identity: album artist + album, exactly as tagged. */
  key: string;
  album: string;
  artist: string;
  year: number | null;
  artworkPath: string | null;
  /** Any track of the album, for artwork resolution. */
  sampleTrackId: number;
  /** In disc/track order; playlist order breaks ties. */
  tracks: TrackRow[];
  totalDurationMs: number;
  /** The album's own rating (0–100) as its tracks carry it; 0 = unrated. */
  rating: number;
  /** Most recent `dateAdded` among the tracks, if any is known. */
  dateAdded: number | null;
}

export const UNKNOWN_ALBUM = 'Unknown Album';
export const UNKNOWN_ARTIST = 'Unknown Artist';

/** Blank tags count as missing, matching the backend's `NULLIF(x, '')`. */
function tagged(value: string | null): string | null {
  const t = value?.trim() ?? '';
  return t === '' ? null : t;
}

/**
 * Group the playlist's rows into albums. Albums appear in the order
 * the playlist first reaches them; inside an album the tracks run
 * 1..N by disc and track number, with the playlist's own order as the
 * tie-break for untagged rows. Untagged rows collect under
 * "Unknown Album" per artist so nothing disappears.
 *
 * Identity is the raw (album artist | artist, album) pair, the same
 * comparison `isAlbumMate` uses when artwork is patched across rows,
 * so a resolved cover lands on exactly the rows in the card.
 */
export function groupByAlbum(rows: readonly TrackRow[]): PlaylistAlbum[] {
  const groups = new Map<string, PlaylistAlbum>();
  for (const t of rows) {
    const artist = tagged(t.albumArtist) ?? tagged(t.artist) ?? UNKNOWN_ARTIST;
    const album = tagged(t.album) ?? UNKNOWN_ALBUM;
    const key = `${artist}\n${album}`;
    let g = groups.get(key);
    if (g === undefined) {
      g = {
        key,
        album,
        artist,
        year: t.year,
        artworkPath: t.artworkPath,
        sampleTrackId: t.id,
        tracks: [],
        totalDurationMs: 0,
        rating: 0,
        dateAdded: null,
      };
      groups.set(key, g);
    }
    g.tracks.push(t);
    g.totalDurationMs += t.durationMs;
    if (g.artworkPath === null && t.artworkPath !== null) g.artworkPath = t.artworkPath;
    if (g.year === null && t.year !== null) g.year = t.year;
    if (t.dateAdded !== null && (g.dateAdded === null || t.dateAdded > g.dateAdded)) {
      g.dateAdded = t.dateAdded;
    }
    // Every track of an album carries the same value; take the first
    // non-zero one in case a stray row lacks it.
    if (g.rating === 0 && t.albumRating > 0) g.rating = t.albumRating;
  }
  for (const g of groups.values()) g.tracks = sortByDiscAndTrack(g.tracks);
  return [...groups.values()];
}

/**
 * Order the cards. Ties keep playlist order (the input order), so a
 * sort by year still reads left-to-right the way the playlist does
 * within a year. Albums lacking the value (no year, unrated, unknown
 * date) go last whichever direction is chosen.
 */
export function sortAlbums(
  albums: readonly PlaylistAlbum[],
  sort: PlaylistAlbumSort,
): PlaylistAlbum[] {
  if (sort.key === 'playlist') return sort.descending ? [...albums].reverse() : [...albums];
  const dir = sort.descending ? -1 : 1;
  const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
  const key = (a: PlaylistAlbum): string | number | null => {
    switch (sort.key) {
      case 'name': {
        return a.album;
      }
      case 'artist': {
        return a.artist;
      }
      case 'year': {
        return a.year;
      }
      case 'rating': {
        return a.rating === 0 ? null : a.rating;
      }
      case 'dateAdded': {
        return a.dateAdded;
      }
      default: {
        return null;
      }
    }
  };
  return albums
    .map((a, i) => ({ a, i, k: key(a) }))
    .sort((x, y) => {
      if (x.k === null || y.k === null) {
        if (x.k === y.k) return x.i - y.i;
        return x.k === null ? 1 : -1;
      }
      const c =
        typeof x.k === 'string' && typeof y.k === 'string'
          ? collator.compare(x.k, y.k)
          : (x.k as number) - (y.k as number);
      return c === 0 ? x.i - y.i : dir * c;
    })
    .map((x) => x.a);
}

/** "★ 4.5" for a 0–100 rating; empty for unrated. */
export function formatRating(rating: number): string {
  if (rating <= 0) return '';
  const stars = Math.round(rating / 2) / 10;
  return `★ ${stars}`;
}

function sortByDiscAndTrack(tracks: TrackRow[]): TrackRow[] {
  const last = Number.MAX_SAFE_INTEGER;
  return tracks
    .map((t, i) => ({ t, i }))
    .sort(
      (a, b) =>
        (a.t.discNumber ?? last) - (b.t.discNumber ?? last) ||
        (a.t.trackNumber ?? last) - (b.t.trackNumber ?? last) ||
        a.i - b.i,
    )
    .map((x) => x.t);
}

/**
 * The per-album presentation of an open playlist: artwork cards that
 * drop down into the album's tracks, so a long playlist is browsed by
 * record rather than as one flat list. Several cards may be open at
 * once — the point is picking songs across albums.
 */
@Component({
  selector: 'app-playlist-album-picker',
  imports: [InViewDirective],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './playlist-album-picker.component.html',
})
export class PlaylistAlbumPickerComponent {
  protected readonly library = inject(LibraryService);
  private readonly playback = inject(PlaybackService);
  private readonly ctx = inject(ContextMenuService);
  private readonly ui = inject(UiService);
  private readonly tauri = inject(TauriService);

  protected readonly albums = computed(this.#computeAlbums.bind(this));

  protected readonly expanded = signal<ReadonlySet<string>>(new Set());

  /** Albums whose artwork was probed this session; misses are remembered. */
  private readonly artworkAttempted = new Set<string>();
  private artworkInFlight = 0;
  private readonly artworkQueue: PlaylistAlbum[] = [];
  /** Concurrent backend lookups; each one reads a file's tags. */
  private static readonly ARTWORK_CONCURRENCY = 4;

  #computeAlbums(): PlaylistAlbum[] {
    return sortAlbums(groupByAlbum(this.library.tracks()), this.ui.playlistAlbumSort());
  }

  protected rating(a: PlaylistAlbum): string {
    return formatRating(a.rating);
  }

  protected trackByAlbum(_index: number, a: PlaylistAlbum): string {
    return a.key;
  }

  protected isExpanded(a: PlaylistAlbum): boolean {
    return this.expanded().has(a.key);
  }

  protected toggle(a: PlaylistAlbum): void {
    this.expanded.update((set) => {
      const next = new Set(set);
      if (next.has(a.key)) next.delete(a.key);
      else next.add(a.key);
      return next;
    });
  }

  /**
   * A card scrolled into view without art: queue a lookup through any
   * of its tracks. `resolveTrackArtwork` patches every album-mate row
   * in `library.tracks`, so the group recomputes with the path.
   */
  protected onCardVisible(a: PlaylistAlbum): void {
    if (a.artworkPath !== null || this.artworkAttempted.has(a.key)) return;
    this.artworkAttempted.add(a.key);
    this.artworkQueue.push(a);
    this.pumpArtworkQueue();
  }

  private pumpArtworkQueue(): void {
    while (
      this.artworkInFlight < PlaylistAlbumPickerComponent.ARTWORK_CONCURRENCY &&
      this.artworkQueue.length > 0
    ) {
      const a = this.artworkQueue.shift()!;
      this.artworkInFlight += 1;
      void this.library
        .resolveTrackArtwork(a.sampleTrackId)
        .catch(() => {
          // A transient failure shouldn't blackhole the album — drop the
          // mark so the next scroll-into-view retries. A settled null
          // (a genuine miss) leaves it in place.
          this.artworkAttempted.delete(a.key);
          return null;
        })
        .finally(() => {
          this.artworkInFlight -= 1;
          this.pumpArtworkQueue();
        });
    }
  }

  protected coverUrl(artworkPath: string | null): string | null {
    if (!artworkPath) return null;
    return convertFileSrc(artworkPath);
  }

  /**
   * The row the player is on, styled like the all-songs list. A synced
   * playlist may list a track twice; only the first copy is marked so
   * a single row is ever "current".
   */
  protected isCurrent(a: PlaylistAlbum, index: number): boolean {
    const id = this.playback.currentTrackId();
    return id !== null && a.tracks.findIndex((x) => x.id === id) === index;
  }

  protected formatDuration(ms: number): string {
    return formatMmSs(ms);
  }

  protected formatTotal(ms: number): string {
    return formatTotalDuration(ms);
  }

  /**
   * Play a row and line up the rest of the card after it, so playback
   * continues 1..N through the album the way the card shows it rather
   * than falling through to stored playlist order. The tail goes ahead
   * of anything already queued, and any of the card's own tracks still
   * queued from an earlier start are dropped first, so re-picking a row
   * never plays part of the album twice. Nothing is queued if the row
   * failed to start.
   */
  protected async playFrom(a: PlaylistAlbum, t: TrackRow): Promise<void> {
    // The menu closure may hold a snapshot; the card can have been
    // regrouped since it opened.
    const album = this.albums().find((x) => x.key === a.key) ?? a;
    let start = album.tracks.indexOf(t);
    if (start === -1) start = album.tracks.findIndex((x) => x.id === t.id);
    if (start === -1) return;
    const started = await this.playback.play(album.tracks[start].id);
    if (!started) return;
    const own = new Set(album.tracks.map((x) => x.id));
    const tail = album.tracks.slice(start + 1);
    this.playback.updateQueue((q) => [...tail, ...q.filter((x) => !own.has(x.id))]);
  }

  protected async playAlbum(a: PlaylistAlbum): Promise<void> {
    if (a.tracks.length === 0) return;
    await this.playFrom(a, a.tracks[0]);
  }

  protected onAlbumContextMenu(a: PlaylistAlbum, event: MouseEvent): void {
    this.ctx.show(event, [
      { label: `Play album (${a.tracks.length})`, action: () => this.playAlbum(a) },
      { label: 'Add album to queue', action: () => this.playback.enqueueAll(a.tracks) },
      { label: 'Play album next', action: () => this.playback.playNextAll(a.tracks) },
    ]);
  }

  protected onTrackContextMenu(a: PlaylistAlbum, t: TrackRow, event: MouseEvent): void {
    this.ctx.show(event, [
      // Same as double-click: the card's remaining tracks follow.
      { label: 'Play', action: () => this.playFrom(a, t) },
      { label: 'Add to queue', action: () => this.playback.enqueue(t) },
      { label: 'Play next', action: () => this.playback.playNext(t) },
      ...this.removeFromPlaylistItems(t),
      { label: '---' },
      { label: 'Get Info…', action: () => this.ui.trackInfo.set({ trackId: t.id }) },
      {
        label: 'Show in Files',
        action: async () => {
          await this.ui.guard(this.tauri.invoke('show_in_files', { trackId: t.id }));
        },
      },
    ]);
  }

  /** Only the user's own, unsynced playlists can be edited in place. */
  private removeFromPlaylistItems(t: TrackRow): ContextMenuItem[] {
    const active = this.library.activePlaylist();
    if (active?.kind !== 'regular' || active.synced) return [];
    return [
      {
        label: 'Remove from Playlist',
        action: async () => {
          await this.ui.guard(this.library.removeTracksFromPlaylist(active.id, [t.id]));
        },
      },
    ];
  }
}
