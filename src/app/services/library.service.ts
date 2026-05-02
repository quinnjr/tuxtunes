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

@Injectable({ providedIn: 'root' })
export class LibraryService {
  private readonly tauri = inject(TauriService);

  readonly stats = signal<LibraryStats | null>(null);
  readonly tracks = signal<TrackRow[]>([]);
  readonly albums = signal<AlbumSummary[]>([]);
  readonly artists = signal<ArtistSummary[]>([]);

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
   * Currently-applied text search. The empty string means "show all";
   * components that mutate this should call `refreshTracks()` to
   * propagate the new filter into `tracks`.
   */
  readonly search = signal<string>('');

  async refreshTracks(limit = 500, offset = 0): Promise<void> {
    const search = this.search().trim() || null;
    const raws = await this.tauri.invoke<TrackRowRaw[]>('list_tracks', {
      limit,
      offset,
      search,
    });
    this.tracks.set(raws.map((raw) => mapTrack(raw)));
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

  async tracksForAlbum(albumArtist: string, album: string): Promise<TrackRow[]> {
    const raws = await this.tauri.invoke<TrackRowRaw[]>('tracks_for_album', {
      albumArtist,
      album,
    });
    return raws.map((raw) => mapTrack(raw));
  }
}
