import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { WindowService } from '../../services/window.service';

/** What the zoom button will do next; drives its glyph, label and click. */
export type ZoomAction = 'maximize' | 'restore' | 'exit-fullscreen';

/**
 * macOS-style traffic lights for the frameless window on Linux and
 * Windows. On macOS the real buttons overlay the toolbar, so this
 * renders nothing there. The zoom button follows the AppKit contract:
 * click toggles maximize, Option/Alt-click enters fullscreen, and once
 * fullscreen a plain click leaves it again (there is no other way out;
 * the app has no F11 binding).
 */
@Component({
  selector: 'app-window-controls',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './window-controls.component.html',
})
export class WindowControlsComponent {
  protected readonly win = inject(WindowService);

  protected readonly zoomAction = computed(this.#computeZoomAction.bind(this));

  protected readonly zoomLabel = computed(this.#computeZoomLabel.bind(this));

  protected close(): void {
    void this.win.close();
  }

  protected minimize(): void {
    void this.win.minimize();
  }

  protected zoom(event: MouseEvent): void {
    if (this.zoomAction() === 'exit-fullscreen' || event.altKey) {
      void this.win.toggleFullscreen();
    } else {
      void this.win.toggleMaximize();
    }
  }

  #computeZoomAction(): ZoomAction {
    if (this.win.fullscreen()) return 'exit-fullscreen';
    return this.win.maximized() ? 'restore' : 'maximize';
  }

  #computeZoomLabel(): string {
    const action = this.zoomAction();
    if (action === 'exit-fullscreen') return 'Exit full screen';
    if (action === 'restore') return 'Restore';
    return 'Zoom (Alt-click: full screen)';
  }
}
