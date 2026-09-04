import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { InViewDirective } from '../../directives/in-view.directive';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { TauriService } from '../../services/tauri.service';
import { UiService } from '../../services/ui.service';
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
      };
      groups.set(key, g);
    }
    g.tracks.push(t);
    g.totalDurationMs += t.durationMs;
    if (g.artworkPath === null && t.artworkPath !== null) g.artworkPath = t.artworkPath;
    if (g.year === null && t.year !== null) g.year = t.year;
  }
  for (const g of groups.values()) g.tracks = sortByDiscAndTrack(g.tracks);
  return [...groups.values()];
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
    return groupByAlbum(this.library.tracks());
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

  /** The row the player is on, styled like the all-songs list. */
  protected isCurrent(t: TrackRow): boolean {
    return this.playback.currentTrackId() === t.id;
  }

  protected formatDuration(ms: number): string {
    return formatMmSs(ms);
  }

  protected formatTotal(ms: number): string {
    return formatTotalDuration(ms);
  }

  protected async play(t: TrackRow): Promise<void> {
    await this.ui.guard(this.playback.play(t.id));
  }

  /**
   * Double-clicking a row plays it and lines up the rest of the card
   * after it, so playback continues 1..N through the album the way the
   * card shows it rather than falling through to stored playlist order.
   */
  protected async playFrom(a: PlaylistAlbum, t: TrackRow): Promise<void> {
    const start = a.tracks.findIndex((x) => x.id === t.id);
    await this.ui.guard(this.playback.play(t.id));
    for (const rest of a.tracks.slice(start + 1)) this.playback.enqueue(rest);
  }

  protected async playAlbum(a: PlaylistAlbum): Promise<void> {
    if (a.tracks.length === 0) return;
    await this.playFrom(a, a.tracks[0]);
  }

  protected onAlbumContextMenu(a: PlaylistAlbum, event: MouseEvent): void {
    this.ctx.show(event, [
      { label: `Play album (${a.tracks.length})`, action: () => this.playAlbum(a) },
      {
        label: 'Add album to queue',
        action: () => {
          for (const t of a.tracks) this.playback.enqueue(t);
        },
      },
      {
        label: 'Play album next',
        action: () => {
          for (const t of [...a.tracks].reverse()) this.playback.playNext(t);
        },
      },
    ]);
  }

  protected onTrackContextMenu(t: TrackRow, event: MouseEvent): void {
    this.ctx.show(event, [
      { label: 'Play', action: () => this.play(t) },
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
