import { Component, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import {
  faFileImport,
  faFolderPlus,
  faGear,
  faPlus,
  faRightFromBracket,
  faWandMagicSparkles,
} from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { WindowService } from '../../services/window.service';
import { WindowControlsComponent } from '../window-controls/window-controls.component';

type MenuId = 'file' | 'settings';

/**
 * The menu bar doubles as the window's title bar: the window is
 * frameless, so its empty area is the drag region (Tauri's injected
 * `data-tauri-drag-region` handler also maps double-click to
 * maximize). On macOS the native traffic lights overlay the left edge,
 * so the bar pads past them instead of drawing its own.
 */
@Component({
  selector: 'app-menu-bar',
  imports: [FaIconComponent, WindowControlsComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './menu-bar.component.html',
})
export class MenuBarComponent {
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);
  protected readonly win = inject(WindowService);

  protected readonly faPlus = faPlus;
  protected readonly faFolderPlus = faFolderPlus;
  protected readonly faWand = faWandMagicSparkles;
  protected readonly faFileImport = faFileImport;
  protected readonly faGear = faGear;
  protected readonly faExit = faRightFromBracket;

  /** Which top-level menu is open, if any. Null closes every dropdown. */
  protected readonly openMenu = signal<MenuId | null>(null);

  protected toggle(menu: MenuId): void {
    this.openMenu.update((m) => (m === menu ? null : menu));
  }

  protected close(): void {
    this.openMenu.set(null);
  }

  protected async addFile(): Promise<void> {
    this.close();
    const summary = await this.ui.guard(this.library.addTracksFromPicker());
    // null: the guard reported a failure, or the dialog was cancelled.
    if (summary === null || summary === undefined) return;
    this.reportSkipped(summary.failed, summary.added.length + summary.existing);
  }

  protected async addFolder(): Promise<void> {
    this.close();
    const summary = await this.ui.guard(this.library.addFolderFromPicker());
    if (summary === null || summary === undefined) return;
    this.reportSkipped(summary.failed, summary.added + summary.skipped);
  }

  /**
   * Say which files were skipped. Without this a selection of
   * unreadable files closes the dialog and does nothing at all, which
   * reads as a broken app.
   */
  private reportSkipped(failed: string[], handled: number): void {
    if (failed.length === 0) return;
    const [first] = failed;
    const name = first.split('/').pop() ?? first;
    const rest = failed.length - 1;
    const tail = rest > 0 ? ` and ${rest} more` : '';
    this.ui.lastError.set(
      handled === 0
        ? `Could not read ${name}${tail}.`
        : `Added ${handled}; could not read ${name}${tail}.`,
    );
  }

  protected async exit(): Promise<void> {
    this.close();
    await this.ui.guard(this.win.quit());
  }

  protected newSmartPlaylist(): void {
    this.close();
    this.ui.smartEditor.set({ playlistId: null });
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
