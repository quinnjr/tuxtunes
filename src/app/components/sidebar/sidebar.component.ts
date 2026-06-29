import { Component, inject, ChangeDetectionStrategy } from '@angular/core';
import { LibraryView, UiService } from '../../services/ui.service';

@Component({
  selector: 'app-sidebar',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './sidebar.component.html',
})
export class SidebarComponent {
  protected readonly ui = inject(UiService);

  /**
   * "Genres" is a tracks view with the Column Browser pre-opened so the
   * user lands on a genre-pivoted list. Other views just switch the
   * top-level libraryView and leave the browser state alone.
   */
  protected setView(view: LibraryView): void {
    if (view === 'genres') {
      this.ui.libraryView.set('tracks');
      this.ui.columnBrowserOpen.set(true);
    } else {
      this.ui.libraryView.set(view);
    }
  }

  protected isActive(view: LibraryView): boolean {
    if (view === 'genres') {
      return this.ui.libraryView() === 'tracks' && this.ui.columnBrowserOpen();
    }
    return this.ui.libraryView() === view && !this.ui.columnBrowserOpen();
  }
}
