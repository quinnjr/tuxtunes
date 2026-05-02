import { Component, inject } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faFileImport, faGear, faPlus } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { LibraryView, UiService } from '../../services/ui.service';

@Component({
  selector: 'app-sidebar',
  imports: [FaIconComponent],
  templateUrl: './sidebar.component.html',
})
export class SidebarComponent {
  protected readonly library = inject(LibraryService);
  protected readonly ui = inject(UiService);
  protected readonly faPlus = faPlus;
  protected readonly faFileImport = faFileImport;
  protected readonly faGear = faGear;

  protected async add(): Promise<void> {
    await this.library.addTrackFromPicker();
  }

  protected openImportWizard(): void {
    this.ui.importWizardOpen.set(true);
  }

  protected openPreferences(): void {
    this.ui.preferencesOpen.set(true);
  }

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
