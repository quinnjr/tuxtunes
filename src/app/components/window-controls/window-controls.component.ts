import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { WindowService } from '../../services/window.service';

/** What the zoom button will do next; drives its glyph, label and click. */
export type ZoomAction = 'maximize' | 'restore' | 'exit-fullscreen';

/**
 * Caption buttons for the frameless window on Linux and Windows, in
 * those desktops' own layout: minimize, maximize, close on the right.
 * On macOS the real traffic lights overlay the toolbar, so this renders
 * nothing there. Maximize toggles maximize; Alt-click enters fullscreen
 * and once fullscreen a plain click leaves it again. F11 (handled in
 * App) toggles fullscreen too, because many Linux window managers grab
 * Alt+click for their own move/resize before the webview sees it.
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
    return 'Maximize (F11 or Alt-click: full screen)';
  }
}
