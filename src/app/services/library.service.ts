import { Injectable, computed, inject, signal } from '@angular/core';
import { TauriService } from './tauri.service';
import { mapTrack, TrackRow, TrackRowRaw } from './playback.service';

export interface LibraryStats {
  trackCount: number;
  totalDurationMs: number;
  totalSizeBytes: number;
}

interface LibraryStatsRaw {
  track_count: number;
  total_duration_ms: number;
  total_size_bytes: number;
}

export interface AlbumSummary {
  album: string;
  albumArtist: string;
  year: number | null;
  trackCount: number;
  totalDurationMs: number;
  artworkPath: string | null;
}

interface AlbumSummaryRaw {
  album: string;
  album_artist: string;
  year: number | null;
  track_count: number;
  total_duration_ms: number;
  artwork_path: string | null;
}

export interface ArtistSummary {
  artist: string;
  albumCount: number;
  trackCount: number;
}

interface ArtistSummaryRaw {
  artist: string;
  album_count: number;
  track_count: number;
}

export type PlaylistKind = 'regular' | 'smart' | 'folder';

export interface Playlist {
  id: number;
  name: string;
  kind: PlaylistKind;
  parentId: number | null;
  sortOrder: number;
  trackCount: number | null;
}

interface PlaylistRaw {
  id: number;
  name: string;
  kind: string;
  parent_id: number | null;
  sort_order: number;
  cached_track_count: number | null;
}

export type DistinctColumn = 'genre' | 'artist' | 'album';

export interface DistinctValue {
  value: string;
  count: number;
}

/**
 * Filters that compose with the track list and distinct queries.
 * `genres`/`artists`/`albums` slots OR within a slot, AND across slots
 * — same shape as the Rust side's TrackFilters.
 */
export interface TrackFilters {
  genres: string[];
  artists: string[];
  albums: string[];
  search: string | null;
}

export const EMPTY_FILTERS: TrackFilters = {
  genres: [],
  artists: [],
  albums: [],
  search: null,
};

export type SortColumn =
  | 'title'
  | 'artist'
  | 'album'
  | 'genre'
  | 'year'
  | 'duration_ms'
  | 'rating'
  | 'play_count'
  | 'last_played'
  | 'date_added'
  | 'bit_rate'
  | 'sample_rate'
  | 'kind'
  | 'size_bytes';

export interface TrackSort {
  column: SortColumn;
  descending: boolean;
}

export const DEFAULT_SORT: TrackSort = {
  column: 'date_added',
  descending: true,
};

@Injectable({ providedIn: 'root' })
export class LibraryService {
  private readonly tauri = inject(TauriService);

  readonly stats = signal<LibraryStats | null>(null);
  readonly tracks = signal<TrackRow[]>([]);
  readonly albums = signal<AlbumSummary[]>([]);
  readonly artists = signal<ArtistSummary[]>([]);
  readonly playlists = signal<Playlist[]>([]);

  /**
   * Playlist currently shown in the track list, or null for the whole
   * library. When set, `refreshTracks()` loads the playlist's tracks
   * (in playlist order) instead of running the filtered library query.
   */
  readonly activePlaylistId = signal<number | null>(null);
  readonly activePlaylist = computed(this.#computeActivePlaylist.bind(this));

  #computeActivePlaylist(): Playlist | null {
    const id = this.activePlaylistId();
    if (id === null) return null;
    return this.playlists().find((p) => p.id === id) ?? null;
  }

  /** Active column-browser + search filters. Drives refreshTracks(). */
  readonly filters = signal<TrackFilters>({ ...EMPTY_FILTERS });

  /** Active sort spec. Header clicks in the track list mutate this. */
  readonly sort = signal<TrackSort>({ ...DEFAULT_SORT });

  /**
   * O(1) id → track lookup, derived from `tracks`. Rebuilt once per
   * `tracks` mutation and cached for every subsequent read, which keeps
   * `currentTrack`-style computeds constant-time even at 100K+ tracks.
   */
  readonly tracksById = computed(this.#computeTracksById.bind(this));

  #computeTracksById(): Map<number, TrackRow> {
    const map = new Map<number, TrackRow>();
    for (const t of this.tracks()) map.set(t.id, t);
    return map;
  }

  async refreshStats(): Promise<void> {
    const raw = await this.tauri.invoke<LibraryStatsRaw>('get_library_stats');
    this.stats.set({
      trackCount: raw.track_count,
      totalDurationMs: raw.total_duration_ms,
      totalSizeBytes: raw.total_size_bytes,
    });
  }

  /**
   * Convenience: the search slot of `filters`, surfaced as a writable
   * signal so the search input doesn't need to know about the rest of
   * the filter shape. Setting this updates `filters` immutably.
   */
  readonly search = signal<string>('');

  setSearch(value: string): void {
    this.search.set(value);
    const trimmed = value.trim();
    this.filters.update((f) => ({ ...f, search: trimmed === '' ? null : trimmed }));
  }

  async refreshTracks(limit = 500, offset = 0): Promise<void> {
    const playlistId = this.activePlaylistId();
    if (playlistId !== null) {
      await this.loadPlaylistTracks(playlistId);
      return;
    }
    const raws = await this.tauri.invoke<TrackRowRaw[]>('list_tracks', {
      limit,
      offset,
      filters: this.filters(),
      sort: this.sort(),
    });
    this.tracks.set(raws.map((raw) => mapTrack(raw)));
  }

  /**
   * Make `id` the active playlist and load its tracks. Passing null
   * returns to the whole-library view. Sort resets to playlist order
   * (the default sort) so the list comes up the way iTunes had it.
   */
  async openPlaylist(id: number | null): Promise<void> {
    this.activePlaylistId.set(id);
    this.sort.set({ ...DEFAULT_SORT });
    await this.refreshTracks();
  }

  /**
   * Playlists are fetched whole (order is stored on the row) and then
   * narrowed client-side by the search box and sort spec, since the
   * backend query has no notion of "the library filtered to a
   * playlist". Filters from the column browser are ignored here — it
   * is closed while a playlist is shown.
   */
  private async loadPlaylistTracks(id: number): Promise<void> {
    const raws = await this.tauri.invoke<TrackRowRaw[]>('open_playlist', { playlistId: id });
    // The user may have switched playlists while the query was in flight.
    if (this.activePlaylistId() !== id) return;
    let rows = raws.map((raw) => mapTrack(raw));
    const search = this.filters().search?.toLowerCase() ?? null;
    if (search !== null) {
      rows = rows.filter((t) =>
        [t.title, t.artist ?? '', t.album ?? ''].some((s) => s.toLowerCase().includes(search)),
      );
    }
    const sort = this.sort();
    if (sort.column !== DEFAULT_SORT.column || sort.descending !== DEFAULT_SORT.descending) {
      rows = sortTracks(rows, sort);
    }
    this.tracks.set(rows);
    const count = raws.length;
    this.playlists.update((all) =>
      all.map((p) => (p.id === id && p.trackCount !== count ? { ...p, trackCount: count } : p)),
    );
  }

  async refreshPlaylists(): Promise<void> {
    const raws = await this.tauri.invoke<PlaylistRaw[]>('list_playlists');
    this.playlists.set(
      raws.map((r) => ({
        id: r.id,
        name: r.name,
        kind: toPlaylistKind(r.kind),
        parentId: r.parent_id,
        sortOrder: r.sort_order,
        trackCount: r.cached_track_count,
      })),
    );
  }

  /**
   * Toggle the sort column. Clicking the active column flips direction;
   * a different column resets to ascending. Refreshes the track list.
   */
  async cycleSort(column: SortColumn): Promise<void> {
    const current = this.sort();
    if (current.column === column) {
      this.sort.set({ column, descending: !current.descending });
    } else {
      this.sort.set({ column, descending: false });
    }
    await this.refreshTracks();
  }

  async getDistinct(column: DistinctColumn): Promise<DistinctValue[]> {
    const raws = await this.tauri.invoke<DistinctValue[]>('get_distinct', {
      column,
      filters: this.filters(),
    });
    return raws;
  }

  async addTrackFromPicker(): Promise<TrackRow | null> {
    const raw = await this.tauri.invoke<TrackRowRaw | null>('pick_and_add_track');
    if (!raw) return null;
    const mapped = mapTrack(raw);
    this.tracks.update((cur) => [mapped, ...cur]);
    await this.refreshStats();
    return mapped;
  }

  async refreshAlbums(): Promise<void> {
    const raws = await this.tauri.invoke<AlbumSummaryRaw[]>('list_albums');
    this.albums.set(
      raws.map((r) => ({
        album: r.album,
        albumArtist: r.album_artist,
        year: r.year,
        trackCount: r.track_count,
        totalDurationMs: r.total_duration_ms,
        artworkPath: r.artwork_path,
      })),
    );
  }

  async refreshArtists(): Promise<void> {
    const raws = await this.tauri.invoke<ArtistSummaryRaw[]>('list_artists');
    this.artists.set(
      raws.map((r) => ({
        artist: r.artist,
        albumCount: r.album_count,
        trackCount: r.track_count,
      })),
    );
  }

  /**
   * Ask the backend to find and cache cover art for an album whose
   * summary has no `artworkPath` yet. Updates the matching entry in
   * `albums` in place on a hit so the grid re-renders that card only.
   */
  async resolveAlbumArtwork(albumArtist: string, album: string): Promise<string | null> {
    const path =
      (await this.tauri.invoke<string | null | undefined>('resolve_album_artwork', {
        albumArtist,
        album,
      })) ?? null;
    if (path !== null) {
      this.albums.update((all) =>
        all.map((a) =>
          a.albumArtist === albumArtist && a.album === album && a.artworkPath !== path
            ? { ...a, artworkPath: path }
            : a,
        ),
      );
    }
    return path;
  }

  async tracksForAlbum(albumArtist: string, album: string): Promise<TrackRow[]> {
    const raws = await this.tauri.invoke<TrackRowRaw[]>('tracks_for_album', {
      albumArtist,
      album,
    });
    return raws.map((raw) => mapTrack(raw));
  }
}

function toPlaylistKind(kind: string): PlaylistKind {
  return kind === 'smart' || kind === 'folder' ? kind : 'regular';
}

/** Comparable value for a sort column; only the columns a TrackRow carries. */
function sortKey(t: TrackRow, column: SortColumn): string | number | null {
  switch (column) {
    case 'title': {
      return t.title;
    }
    case 'artist': {
      return t.artist;
    }
    case 'album': {
      return t.album;
    }
    case 'duration_ms': {
      return t.durationMs;
    }
    case 'play_count': {
      return t.playCount;
    }
    case 'sample_rate': {
      return t.sampleRate;
    }
    case 'kind': {
      return t.kind;
    }
    default: {
      return null;
    }
  }
}

/**
 * Stable client-side sort mirroring the backend's semantics: strings
 * compare case-insensitively, nulls sort last ascending / first
 * descending. Unknown columns leave the order untouched.
 */
export function sortTracks(rows: TrackRow[], sort: TrackSort): TrackRow[] {
  const dir = sort.descending ? -1 : 1;
  return rows
    .map((t, i) => ({ t, i }))
    .sort((a, b) => {
      const ka = sortKey(a.t, sort.column);
      const kb = sortKey(b.t, sort.column);
      if (ka === null && kb === null) return a.i - b.i;
      // Nulls last ascending, first descending (matches the SQL side).
      if (ka === null) return dir;
      if (kb === null) return -dir;
      let cmp: number;
      if (typeof ka === 'string' && typeof kb === 'string') {
        cmp = ka.localeCompare(kb, undefined, { sensitivity: 'base' });
      } else {
        cmp = (ka as number) < (kb as number) ? -1 : (ka as number) > (kb as number) ? 1 : 0;
      }
      return cmp === 0 ? a.i - b.i : cmp * dir;
    })
    .map((x) => x.t);
}
