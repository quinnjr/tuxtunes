import { Component, HostListener, inject } from '@angular/core';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';

@Component({
  selector: 'app-context-menu',
  imports: [],
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

  protected isDivider(item: ContextMenuItem): boolean {
    return item.label === '---';
  }

  protected async onItemClick(item: ContextMenuItem): Promise<void> {
    await this.ctx.run(item);
  }
}
