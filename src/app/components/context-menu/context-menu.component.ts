import {
  Component,
  ElementRef,
  HostListener,
  effect,
  inject,
  signal,
  viewChild,
  ChangeDetectionStrategy,
} from '@angular/core';
import {
  ROVING_FOCUS_ORIENTATION,
  RovingFocusDirective,
} from '../../directives/roving-focus.directive';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';

@Component({
  selector: 'app-context-menu',
  imports: [RovingFocusDirective],
  // A menu is vertical: Up/Down move, Left/Right belong to submenu open/close.
  providers: [{ provide: ROVING_FOCUS_ORIENTATION, useValue: 'vertical' as const }],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './context-menu.component.html',
})
export class ContextMenuComponent {
  protected readonly ctx = inject(ContextMenuService);

  /** ESC dismisses. Click anywhere else also dismisses (handled in template). */
  @HostListener('document:keydown.escape')
  onEscape(): void {
    this.ctx.hide();
  }

  /**
   * Right-click on the backdrop should also dismiss — without this the
   * native browser menu would fire and the app menu would stay open.
   */
  @HostListener('document:contextmenu', ['$event'])
  onContext(event: MouseEvent): void {
    if (this.ctx.open() === null) return;
    // Let the consumer-side oncontextmenu handler call `show()` first;
    // if open() is still set after that microtask, hide it here. The
    // simplest approach is to dismiss only when the event target isn't
    // inside the open menu.
    const target = event.target as HTMLElement | null;
    if (target?.closest('[data-context-menu]')) return;
    this.ctx.hide();
  }

  /** Index (into the open menu's items) of the expanded submenu. */
  protected readonly submenuIndex = signal<number | null>(null);

  /**
   * The menu element, focused on open. A context menu is keyboard-first:
   * without this the arrows would have nothing to move from and the menu
   * would be mouse-only despite `role="menu"`.
   */
  private readonly menuEl = viewChild<ElementRef<HTMLElement>>('menu');

  /** Pending grace-delay close of the flyout (hover intent). */
  private closeTimer: ReturnType<typeof setTimeout> | null = null;
  private static readonly SUBMENU_CLOSE_DELAY_MS = 300;

  constructor() {
    // A freshly opened (or closed) menu must never inherit the last
    // one's expanded flyout.
    effect(() => {
      const open = this.ctx.open();
      this.cancelPendingClose();
      this.submenuIndex.set(null);
      if (open !== null) {
        // Move focus to the first item once the menu is in the DOM (APG:
        // a menu opens with focus on its first item).
        setTimeout(() => this.firstItem()?.focus());
      }
    });
  }

  private firstItem(): HTMLElement | null {
    return this.menuEl()?.nativeElement.querySelector<HTMLElement>('[role^="menuitem"]') ?? null;
  }

  protected isDivider(item: ContextMenuItem): boolean {
    return item.label === '---';
  }

  protected hasChildren(item: ContextMenuItem): boolean {
    return (item.children?.length ?? 0) > 0;
  }

  /**
   * True when any item in the list is checkable — only then does the
   * menu reserve a checkmark gutter, so plain menus don't indent.
   */
  protected hasChecks(items: ContextMenuItem[]): boolean {
    return items.some((i) => i.checked !== undefined);
  }

  /**
   * Whether the flyout should open to the left: near the right viewport
   * edge there is no room for menu (≈200px) + flyout (≈180px).
   */
  protected flyoutFlipped(x: number): boolean {
    return x > window.innerWidth - 400;
  }

  /**
   * Hovering a top-level item opens its submenu immediately, but a
   * childless sibling only *schedules* the close — the natural diagonal
   * move toward a flyout entry brushes siblings, and an instant close
   * would slam the flyout shut mid-gesture. Re-entering the flyout (or
   * the parent) cancels the pending close.
   */
  protected onItemEnter(index: number, item: ContextMenuItem): void {
    // A disabled entry must not react at all — no flyout, no close
    // scheduling. (It stays focusable so AT can announce it — see the
    // `aria-disabled` note on the template.)
    if (item.disabled) return;
    if (this.hasChildren(item)) {
      this.cancelPendingClose();
      this.submenuIndex.set(index);
      return;
    }
    if (this.submenuIndex() === null) return;
    this.closeTimer ??= setTimeout(() => {
      this.closeTimer = null;
      this.submenuIndex.set(null);
    }, ContextMenuComponent.SUBMENU_CLOSE_DELAY_MS);
  }

  protected onSubmenuEnter(): void {
    this.cancelPendingClose();
  }

  private cancelPendingClose(): void {
    if (this.closeTimer !== null) {
      clearTimeout(this.closeTimer);
      this.closeTimer = null;
    }
  }

  protected async onItemClick(index: number, item: ContextMenuItem): Promise<void> {
    // A disabled item is a no-op: it must not dismiss the menu (that is
    // what `run()` would do before its own disabled guard) nor open a
    // flyout.
    if (item.disabled) return;
    if (this.hasChildren(item)) {
      this.cancelPendingClose();
      this.submenuIndex.set(index);
      return;
    }
    this.submenuIndex.set(null);
    await this.ctx.run(item);
  }

  protected async onChildClick(item: ContextMenuItem): Promise<void> {
    if (item.disabled) return;
    this.submenuIndex.set(null);
    await this.ctx.run(item);
  }

  /**
   * Menu-model keys beyond the roving Up/Down. On a top-level item,
   * ArrowRight opens its flyout and moves focus to its first item; Enter
   * or Space on a parent does the same. Inside a flyout, ArrowLeft closes
   * it and returns focus to the parent. ArrowLeft on a top-level item is
   * left alone (nothing to close).
   */
  protected onMenuKeydown(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    const inSubmenu = target?.closest('[data-submenu]') !== null && target !== null;

    if (inSubmenu) {
      if (event.key === 'ArrowLeft') {
        event.preventDefault();
        this.closeSubmenuAndRefocusParent();
      }
      return;
    }

    const item = this.itemAt(event);
    if (item === null) return;
    // A disabled parent must not open its flyout by keyboard either — the
    // click and hover paths guard this; the key path must too.
    if (item.disabled) return;

    const opensSubmenu =
      this.hasChildren(item) &&
      (event.key === 'ArrowRight' || event.key === 'Enter' || event.key === ' ');
    if (opensSubmenu) {
      event.preventDefault();
      this.openSubmenu(this.indexOf(item));
      return;
    }
    if (event.key === 'ArrowRight') {
      // A childless item is not a submenu parent; swallow the key rather
      // than let it fall through to nothing.
      event.preventDefault();
    }
  }

  /** Expand item `index`'s flyout and move focus into it. */
  private openSubmenu(index: number): void {
    this.cancelPendingClose();
    this.submenuIndex.set(index);
    // The flyout renders after this tick; focus its first item then.
    setTimeout(() => {
      this.menuEl()
        ?.nativeElement.querySelector<HTMLElement>('[data-submenu] [role^="menuitem"]')
        ?.focus();
    });
  }

  /** Collapse the open flyout and put focus back on its parent item. */
  private closeSubmenuAndRefocusParent(): void {
    const index = this.submenuIndex();
    this.cancelPendingClose();
    this.submenuIndex.set(null);
    if (index === null) return;
    const parent = this.menuEl()?.nativeElement.querySelector<HTMLElement>(
      `[data-item-index="${index}"]`,
    );
    parent?.focus();
  }

  private itemAt(event: KeyboardEvent): ContextMenuItem | null {
    const state = this.ctx.open();
    if (state === null) return null;
    const target = event.target as HTMLElement | null;
    const index = Number(target?.getAttribute('data-item-index') ?? Number.NaN);
    return Number.isNaN(index) ? null : (state.items[index] ?? null);
  }

  private indexOf(item: ContextMenuItem): number {
    return this.ctx.open()?.items.indexOf(item) ?? -1;
  }
}
