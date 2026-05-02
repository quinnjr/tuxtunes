import { Component, inject } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { LibraryView, UiService } from '../../services/ui.service';
import { AlbumGridViewComponent } from '../album-grid-view/album-grid-view.component';
import { ArtistSplitViewComponent } from '../artist-split-view/artist-split-view.component';
import { ColumnBrowserComponent } from '../column-browser/column-browser.component';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';
import { TrackListViewComponent } from '../track-list-view/track-list-view.component';

@Component({
  selector: 'app-main-content',
  imports: [
    AlbumGridViewComponent,
    ArtistSplitViewComponent,
    ColumnBrowserComponent,
    SettingsAudioComponent,
    TrackListViewComponent,
  ],
  templateUrl: './main-content.component.html',
})
export class MainContentComponent {
  protected readonly ui = inject(UiService);
  protected readonly library = inject(LibraryService);
  protected readonly viewMode = this.ui.libraryView;
  protected readonly modes: readonly LibraryView[] = [
    'tracks',
    'albums',
    'artists',
    'settings',
  ] as const;

  private searchTimer: ReturnType<typeof setTimeout> | null = null;

  protected setMode(mode: LibraryView): void {
    this.ui.libraryView.set(mode);
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
      void this.library.refreshTracks();
      this.searchTimer = null;
    }, 200);
  }

  protected clearSearch(): void {
    this.library.setSearch('');
    void this.library.refreshTracks();
  }
}
