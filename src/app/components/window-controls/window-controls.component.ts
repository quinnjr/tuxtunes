import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { WindowService } from '../../services/window.service';

/**
 * macOS-style traffic lights for the frameless window on Linux and
 * Windows. On macOS the real buttons overlay the toolbar, so this
 * renders nothing there. The zoom button follows the AppKit contract:
 * click toggles maximize, Option/Alt-click toggles fullscreen.
 */
@Component({
  selector: 'app-window-controls',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './window-controls.component.html',
})
export class WindowControlsComponent {
  protected readonly win = inject(WindowService);

  protected close(): void {
    void this.win.close();
  }

  protected minimize(): void {
    void this.win.minimize();
  }

  protected zoom(event: MouseEvent): void {
    if (event.altKey) {
      void this.win.toggleFullscreen();
    } else {
      void this.win.toggleMaximize();
    }
  }
}
