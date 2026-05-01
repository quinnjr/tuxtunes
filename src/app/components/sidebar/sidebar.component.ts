import { Component, inject } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faFileImport, faGear, faPlus } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';

@Component({
  selector: 'app-sidebar',
  imports: [FaIconComponent],
  templateUrl: './sidebar.component.html',
})
export class SidebarComponent {
  protected readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);
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
}
