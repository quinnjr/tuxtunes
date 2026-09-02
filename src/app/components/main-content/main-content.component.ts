import { Component, OnDestroy, inject, ChangeDetectionStrategy } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { LibraryView, PlaylistView, UiService } from '../../services/ui.service';
import { AlbumGridViewComponent } from '../album-grid-view/album-grid-view.component';
import { ArtistSplitViewComponent } from '../artist-split-view/artist-split-view.component';
import { ColumnBrowserComponent } from '../column-browser/column-browser.component';
import { PlaylistAlbumPickerComponent } from '../playlist-album-picker/playlist-album-picker.component';
import { TrackListViewComponent } from '../track-list-view/track-list-view.component';

@Component({
  selector: 'app-main-content',
  imports: [
    AlbumGridViewComponent,
    ArtistSplitViewComponent,
    ColumnBrowserComponent,
    PlaylistAlbumPickerComponent,
    TrackListViewComponent,
  ],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './main-content.component.html',
})
export class MainContentComponent implements OnDestroy {
  protected readonly ui = inject(UiService);
  protected readonly library = inject(LibraryService);
  protected readonly viewMode = this.ui.libraryView;
  protected readonly modes: readonly LibraryView[] = ['tracks', 'albums', 'artists'] as const;
  protected readonly playlistModes: readonly PlaylistView[] = ['albums', 'songs'] as const;

  private searchTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * The device view replaces this component rather than nesting inside
   * it, so switching to a device destroys it. A pending search debounce
   * would otherwise fire afterwards and refresh a list nothing is
   * showing.
   */
  ngOnDestroy(): void {
    if (this.searchTimer !== null) clearTimeout(this.searchTimer);
    this.searchTimer = null;
  }

  /**
   * The segmented control always addresses the whole library, so
   * picking a mode leaves any active playlist. Switching to "tracks"
   * needs an explicit reload because the track list only fetches on
   * mount and the view may already be 'tracks'.
   */
  protected setMode(mode: LibraryView): void {
    const hadPlaylist = this.library.activePlaylistId() !== null;
    this.library.activePlaylistId.set(null);
    this.ui.libraryView.set(mode);
    if (hadPlaylist && mode === 'tracks') void this.ui.guard(this.library.refreshTracks());
  }

  /** Switch how the open playlist is presented; the rows are shared. */
  protected setPlaylistMode(mode: PlaylistView): void {
    this.ui.playlistView.set(mode);
  }

  protected toggleBrowser(): void {
    this.ui.columnBrowserOpen.update((v) => !v);
  }

  /**
   * Debounce search-box input by 200ms before re-running list_tracks.
   * Avoids a query per keystroke while still feeling instant.
   */
  protected onSearchInput(event: Event): void {
    const value = (event.target as HTMLInputElement).value;
    this.library.setSearch(value);
    if (this.searchTimer !== null) clearTimeout(this.searchTimer);
    this.searchTimer = setTimeout(() => {
      void this.ui.guard(this.library.refreshTracks());
      this.searchTimer = null;
    }, 200);
  }

  protected clearSearch(): void {
    this.library.setSearch('');
    void this.ui.guard(this.library.refreshTracks());
  }
}
