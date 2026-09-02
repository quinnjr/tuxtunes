import { Injectable, signal } from '@angular/core';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow, type Window } from '@tauri-apps/api/window';

export type Platform = 'linux' | 'macos' | 'windows';

/** The eight resize handles a frameless window needs. Mirrors Tauri's `ResizeDirection` enum. */
export type ResizeDirection =
  'North' | 'South' | 'East' | 'West' | 'NorthEast' | 'NorthWest' | 'SouthEast' | 'SouthWest';

/**
 * Tauri's os plugin would answer this too, but it costs a Rust plugin
 * plus a permission for one string; every webview we ship (WebKitGTK,
 * WKWebView, WebView2) already puts the OS in the user agent.
 */
export function detectPlatform(userAgent: string): Platform {
  if (/Macintosh|Mac OS X/i.test(userAgent)) return 'macos';
  if (/Windows/i.test(userAgent)) return 'windows';
  return 'linux';
}

/**
 * The window is frameless (`decorations: false`) on Linux and Windows,
 * so the UI draws its own title bar, traffic lights and, on Linux,
 * resize edges. macOS keeps native decorations in overlay mode: the
 * real traffic lights float over our toolbar and AppKit handles resize.
 */
@Injectable({ providedIn: 'root' })
export class WindowService {
  readonly platform: Platform = detectPlatform(globalThis.navigator?.userAgent ?? '');

  /** False under plain `ng serve` in a browser; every control hides. */
  readonly available: boolean = isTauri();

  /** We draw close/minimize/zoom ourselves everywhere but macOS. */
  readonly customControls: boolean = this.available && this.platform !== 'macos';

  /** Only Linux lacks native resize edges on a frameless window. */
  readonly customResize: boolean = this.available && this.platform === 'linux';

  readonly maximized = signal(false);
  readonly fullscreen = signal(false);

  private readonly win: Window | null = this.available ? getCurrentWindow() : null;

  constructor() {
    if (!this.win) return;
    void this.refreshState();
    void this.win.onResized(() => void this.refreshState());
  }

  async startDragging(): Promise<void> {
    await this.win?.startDragging();
  }

  async startResizeDragging(direction: ResizeDirection): Promise<void> {
    await this.win?.startResizeDragging(direction);
  }

  async minimize(): Promise<void> {
    await this.win?.minimize();
  }

  async toggleMaximize(): Promise<void> {
    await this.win?.toggleMaximize();
    await this.refreshState();
  }

  async toggleFullscreen(): Promise<void> {
    if (!this.win) return;
    await this.win.setFullscreen(!this.fullscreen());
    await this.refreshState();
  }

  async close(): Promise<void> {
    await this.win?.close();
  }

  private async refreshState(): Promise<void> {
    if (!this.win) return;
    const [maximized, fullscreen] = await Promise.all([
      this.win.isMaximized(),
      this.win.isFullscreen(),
    ]);
    this.maximized.set(Boolean(maximized));
    this.fullscreen.set(Boolean(fullscreen));
  }
}
