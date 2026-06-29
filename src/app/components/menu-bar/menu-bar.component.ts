import { Component, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faFileImport, faGear, faPlus } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';

type MenuId = 'file' | 'settings';

@Component({
  selector: 'app-menu-bar',
  imports: [FaIconComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './menu-bar.component.html',
})
export class MenuBarComponent {
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);

  protected readonly faPlus = faPlus;
  protected readonly faFileImport = faFileImport;
  protected readonly faGear = faGear;

  /** Which top-level menu is open, if any. Null closes every dropdown. */
  protected readonly openMenu = signal<MenuId | null>(null);

  protected toggle(menu: MenuId): void {
    this.openMenu.update((m) => (m === menu ? null : menu));
  }

  protected close(): void {
    this.openMenu.set(null);
  }

  protected async addFiles(): Promise<void> {
    this.close();
    await this.library.addTrackFromPicker();
  }

  protected importItunes(): void {
    this.close();
    this.ui.importWizardOpen.set(true);
  }

  protected openPreferences(): void {
    this.close();
    this.ui.preferencesOpen.set(true);
  }
}
