import { Injectable, signal } from '@angular/core';

export interface ContextMenuItem {
  /** Display label. Use `---` as a divider. */
  label: string;
  action?: () => void | Promise<void>;
  /** Render the item as a destructive variant (red-ish accent). */
  destructive?: boolean;
  /** Disabled items are visible but not clickable. */
  disabled?: boolean;
}

export interface ContextMenuState {
  x: number;
  y: number;
  items: ContextMenuItem[];
}

@Injectable({ providedIn: 'root' })
export class ContextMenuService {
  /** Currently-open menu, or null when nothing is shown. */
  readonly open = signal<ContextMenuState | null>(null);

  /**
   * Show the menu at the event's screen position. The caller passes the
   * action items; the service handles geometry, dismissal, and ESC.
   */
  show(event: MouseEvent, items: ContextMenuItem[]): void {
    event.preventDefault();
    this.open.set({ x: event.clientX, y: event.clientY, items });
  }

  hide(): void {
    this.open.set(null);
  }

  async run(item: ContextMenuItem): Promise<void> {
    this.hide();
    if (item.disabled) return;
    if (item.action) await item.action();
  }
}
