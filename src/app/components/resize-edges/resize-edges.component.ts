import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { type ResizeDirection, WindowService } from '../../services/window.service';

interface Edge {
  dir: ResizeDirection;
  class: string;
}

/**
 * Invisible resize handles around the frameless window. Linux only:
 * WebKitGTK gives a decorations-less window no resize border, so the
 * compositor has to be asked to start a resize from a mousedown on our
 * side. Hidden while maximized or fullscreen, where resizing is moot
 * and the handles would steal the outermost pixel of the content.
 */
@Component({
  selector: 'app-resize-edges',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './resize-edges.component.html',
})
export class ResizeEdgesComponent {
  protected readonly win = inject(WindowService);

  /** Cursor names are the CSS ones; the Tauri direction is what the compositor gets. */
  protected readonly edges: readonly Edge[] = [
    { dir: 'North', class: 'top-0 left-2 right-2 h-1 cursor-n-resize' },
    { dir: 'South', class: 'bottom-0 left-2 right-2 h-1 cursor-s-resize' },
    { dir: 'West', class: 'left-0 top-2 bottom-2 w-1 cursor-w-resize' },
    { dir: 'East', class: 'right-0 top-2 bottom-2 w-1 cursor-e-resize' },
    { dir: 'NorthWest', class: 'top-0 left-0 h-2 w-2 cursor-nw-resize' },
    { dir: 'NorthEast', class: 'top-0 right-0 h-2 w-2 cursor-ne-resize' },
    { dir: 'SouthWest', class: 'bottom-0 left-0 h-2 w-2 cursor-sw-resize' },
    { dir: 'SouthEast', class: 'bottom-0 right-0 h-2 w-2 cursor-se-resize' },
  ];

  protected get active(): boolean {
    return this.win.customResize && !this.win.maximized() && !this.win.fullscreen();
  }

  protected onMouseDown(event: MouseEvent, dir: ResizeDirection): void {
    if (event.button !== 0) return;
    event.preventDefault();
    void this.win.startResizeDragging(dir);
  }
}
