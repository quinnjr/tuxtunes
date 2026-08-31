import {
  Component,
  HostListener,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';

@Component({
  selector: 'app-context-menu',
  imports: [],
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

  /** Pending grace-delay close of the flyout (hover intent). */
  private closeTimer: ReturnType<typeof setTimeout> | null = null;
  private static readonly SUBMENU_CLOSE_DELAY_MS = 300;

  constructor() {
    // A freshly opened (or closed) menu must never inherit the last
    // one's expanded flyout.
    effect(() => {
      this.ctx.open();
      this.cancelPendingClose();
      this.submenuIndex.set(null);
    });
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
    if (this.hasChildren(item)) {
      this.cancelPendingClose();
      this.submenuIndex.set(index);
      return;
    }
    this.submenuIndex.set(null);
    await this.ctx.run(item);
  }

  protected async onChildClick(item: ContextMenuItem): Promise<void> {
    this.submenuIndex.set(null);
    await this.ctx.run(item);
  }
}
