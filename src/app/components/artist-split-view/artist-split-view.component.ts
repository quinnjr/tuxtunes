import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { LibraryService, AlbumSummary, ArtistSummary } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-artist-split-view',
  imports: [],
  templateUrl: './artist-split-view.component.html',
})
export class ArtistSplitViewComponent implements OnInit {
  protected readonly library = inject(LibraryService);
  private readonly playback = inject(PlaybackService);

  protected readonly selected = signal<string | null>(null);
  protected readonly tracks = signal<TrackRow[]>([]);

  /** Albums of the selected artist, derived from the global album list. */
  protected readonly albumsForSelected = computed(this.#computeAlbumsForSelected.bind(this));

  ngOnInit(): void {
    // Albums also feeds the right-hand pane filter; refresh both in parallel.
    void Promise.all([this.library.refreshArtists(), this.library.refreshAlbums()]);
  }

  #computeAlbumsForSelected(): AlbumSummary[] {
    const a = this.selected();
    if (a === null) return [];
    return this.library.albums().filter((al) => al.albumArtist === a);
  }

  protected trackByArtist(_index: number, a: ArtistSummary): string {
    return a.artist;
  }

  protected trackByAlbum(_index: number, a: AlbumSummary): string {
    return `${a.albumArtist} ${a.album}`;
  }

  protected trackByTrack(_index: number, t: TrackRow): number {
    return t.id;
  }

  protected async select(a: ArtistSummary): Promise<void> {
    this.selected.set(a.artist);
    // "All tracks for this artist" — concatenate every album's tracks.
    const albums = this.library.albums().filter((al) => al.albumArtist === a.artist);
    const lists = await Promise.all(
      albums.map((al) => this.library.tracksForAlbum(al.albumArtist, al.album)),
    );
    this.tracks.set(lists.flat());
  }

  protected async play(t: TrackRow): Promise<void> {
    await this.playback.play(t.id);
  }

  protected formatDuration(ms: number): string {
    return formatMmSs(ms);
  }
}
