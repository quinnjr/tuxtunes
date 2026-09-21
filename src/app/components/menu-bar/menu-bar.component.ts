import {
  Component,
  ElementRef,
  HostListener,
  OnDestroy,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import {
  faFileImport,
  faFolderPlus,
  faGear,
  faPlus,
  faRightFromBracket,
  faSliders,
  faWandMagicSparkles,
} from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import {
  ROVING_FOCUS_ORIENTATION,
  RovingFocusDirective,
} from '../../directives/roving-focus.directive';
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
  imports: [FaIconComponent, WindowControlsComponent, RovingFocusDirective],
  // Dropdowns are vertical; Left/Right are handled by onMenuKeydown to
  // move between the top-level menus.
  providers: [{ provide: ROVING_FOCUS_ORIENTATION, useValue: 'vertical' as const }],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './menu-bar.component.html',
})
export class MenuBarComponent implements OnDestroy {
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  protected readonly win = inject(WindowService);

  protected readonly faPlus = faPlus;
  protected readonly faFolderPlus = faFolderPlus;
  protected readonly faWand = faWandMagicSparkles;
  protected readonly faFileImport = faFileImport;
  protected readonly faGear = faGear;
  protected readonly faSliders = faSliders;
  protected readonly faExit = faRightFromBracket;

  /** Which top-level menu is open, if any. Null closes every dropdown. */
  protected readonly openMenu = signal<MenuId | null>(null);

  private focusTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    // Mirror the open dropdown into UiService so the shared shortcut
    // guard suppresses list shortcuts behind it.
    effect(() => this.ui.menubarOpen.set(this.openMenu() !== null));
    effect(() => {
      const open = this.openMenu();
      if (this.focusTimer !== null) {
        clearTimeout(this.focusTimer);
        this.focusTimer = null;
      }
      if (open === null) return;
      // Move focus into the opened dropdown so its keys reach the items;
      // scoped to this component's own subtree, not the document.
      this.focusTimer = setTimeout(() => {
        this.focusTimer = null;
        this.host.nativeElement
          .querySelector<HTMLElement>(`[data-menu-dropdown="${open}"] [role="menuitem"]`)
          ?.focus();
      });
    });
  }

  ngOnDestroy(): void {
    if (this.focusTimer !== null) clearTimeout(this.focusTimer);
  }

  /** The trigger button for a menu, so close can hand focus back. */
  private trigger(menu: MenuId): HTMLElement | null {
    return this.host.nativeElement.querySelector<HTMLElement>(`[data-menu-trigger="${menu}"]`);
  }

  protected toggle(menu: MenuId): void {
    this.openMenu.update((m) => (m === menu ? null : menu));
  }

  protected close(): void {
    const menu = this.openMenu();
    this.openMenu.set(null);
    // Hand focus back to the trigger (APG: Escape returns focus to the
    // top-level item). Deferred: the dropdown is removed after this.
    // Conditional: a menu item action may open a modal in the same tick,
    // which moves focus into itself — restoring here would pull focus
    // back out from behind it.
    if (menu !== null) {
      const trigger = this.trigger(menu);
      const doc = this.host.nativeElement.ownerDocument;
      queueMicrotask(() => {
        if (doc.activeElement !== doc.body) return;
        if (trigger === null || !trigger.isConnected || trigger.closest('[inert]') !== null) {
          return;
        }
        trigger.focus();
      });
    }
  }

  /**
   * ArrowLeft/ArrowRight cycle between the top-level menus (the menu
   * bar's horizontal axis, APG menubar pattern). The focus-in effect
   * carries focus into the newly opened dropdown. Up/Down are the roving
   * directive's job, scoped to `vertical` for the dropdowns so it does
   * not also act on these keys.
   */
  protected onMenuKeydown(event: KeyboardEvent): void {
    const order: MenuId[] = ['file', 'settings'];
    const current = order.indexOf(this.openMenu() ?? 'file');
    if (event.key === 'ArrowRight') {
      event.preventDefault();
      this.openMenu.set(order[(current + 1) % order.length]);
    } else if (event.key === 'ArrowLeft') {
      event.preventDefault();
      this.openMenu.set(order[(current - 1 + order.length) % order.length]);
    }
  }

  /**
   * Escape closes an open menu. Bound on document, not on the
   * click-catcher: that div has no tabindex and is not an ancestor of
   * the menu, so a keydown never reaches it — the same reason
   * ContextMenuComponent binds this on the host.
   */
  @HostListener('document:keydown.escape')
  onEscape(): void {
    this.close();
  }

  /**
   * Ctrl+Q quits, the accelerator every Linux desktop uses. Suppressed
   * while typing so it cannot fire from the search box.
   */
  @HostListener('document:keydown.control.q', ['$event'])
  onQuitShortcut(event: Event): void {
    const el = event.target as HTMLElement | null;
    if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) {
      return;
    }
    event.preventDefault();
    void this.exit();
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

  protected openSettings(): void {
    this.close();
    this.ui.settingsOpen.set(true);
  }
}
