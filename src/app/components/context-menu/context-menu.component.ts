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

  constructor() {
    // A freshly opened (or closed) menu must never inherit the last
    // one's expanded flyout.
    effect(() => {
      this.ctx.open();
      this.submenuIndex.set(null);
    });
  }

  protected isDivider(item: ContextMenuItem): boolean {
    return item.label === '---';
  }

  protected hasChildren(item: ContextMenuItem): boolean {
    return (item.children?.length ?? 0) > 0;
  }

  /** Hovering any top-level item opens its submenu or closes a stale one. */
  protected onItemEnter(index: number, item: ContextMenuItem): void {
    this.submenuIndex.set(this.hasChildren(item) ? index : null);
  }

  protected async onItemClick(index: number, item: ContextMenuItem): Promise<void> {
    if (this.hasChildren(item)) {
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
