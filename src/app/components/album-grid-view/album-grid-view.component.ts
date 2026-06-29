import { Component, OnInit, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { ContextMenuService } from '../../services/context-menu.service';
import { LibraryService, AlbumSummary } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-album-grid-view',
  imports: [],
  changeDetection: ChangeDetectionStrategy.Eager,
  templateUrl: './album-grid-view.component.html',
})
export class AlbumGridViewComponent implements OnInit {
  protected readonly library = inject(LibraryService);
  private readonly playback = inject(PlaybackService);
  private readonly ctx = inject(ContextMenuService);

  /** Currently-expanded album. Clicking a card flips its inline track list. */
  protected readonly expanded = signal<{ albumArtist: string; album: string } | null>(null);
  protected readonly expandedTracks = signal<TrackRow[]>([]);

  ngOnInit(): void {
    void this.library.refreshAlbums();
  }

  protected trackByAlbum(_index: number, a: AlbumSummary): string {
    return `${a.albumArtist} ${a.album}`;
  }

  protected trackByTrack(_index: number, t: TrackRow): number {
    return t.id;
  }

  protected isExpanded(a: AlbumSummary): boolean {
    const e = this.expanded();
    return e !== null && e.albumArtist === a.albumArtist && e.album === a.album;
  }

  protected async toggle(a: AlbumSummary): Promise<void> {
    if (this.isExpanded(a)) {
      this.expanded.set(null);
      this.expandedTracks.set([]);
      return;
    }
    this.expanded.set({ albumArtist: a.albumArtist, album: a.album });
    const tracks = await this.library.tracksForAlbum(a.albumArtist, a.album);
    this.expandedTracks.set(tracks);
  }

  protected async play(t: TrackRow): Promise<void> {
    await this.playback.play(t.id);
  }

  protected formatDuration(ms: number): string {
    return formatMmSs(ms);
  }

  /** Convert a Tauri-side file path into an asset URL the webview can load. */
  protected coverUrl(artworkPath: string | null): string | null {
    if (!artworkPath) return null;
    return convertFileSrc(artworkPath);
  }

  /**
   * Right-click an album card → load its tracks (if not already loaded)
   * and offer enqueue / play-next / play actions over the whole album.
   * Falls back to a single-track menu if a row is right-clicked inside
   * the expanded list.
   */
  protected async onAlbumContextMenu(a: AlbumSummary, event: MouseEvent): Promise<void> {
    const tracks =
      this.isExpanded(a) && this.expandedTracks().length > 0
        ? this.expandedTracks()
        : await this.library.tracksForAlbum(a.albumArtist, a.album);
    this.ctx.show(event, [
      {
        label: `Play album (${tracks.length})`,
        action: async () => {
          if (tracks.length === 0) return;
          await this.playback.play(tracks[0].id);
          for (let i = 1; i < tracks.length; i += 1) this.playback.enqueue(tracks[i]);
        },
      },
      {
        label: `Add album to queue`,
        action: () => {
          for (const t of tracks) this.playback.enqueue(t);
        },
      },
      {
        label: `Play album next`,
        action: () => {
          for (const t of [...tracks].reverse()) this.playback.playNext(t);
        },
      },
    ]);
  }

  protected onTrackContextMenu(t: TrackRow, event: MouseEvent): void {
    this.ctx.show(event, [
      { label: 'Play', action: () => void this.play(t) },
      { label: 'Add to queue', action: () => this.playback.enqueue(t) },
      { label: 'Play next', action: () => this.playback.playNext(t) },
    ]);
  }
}
