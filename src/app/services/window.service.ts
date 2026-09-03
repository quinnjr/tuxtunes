import { Injectable, computed, inject, signal } from '@angular/core';
import { isTauri } from '@tauri-apps/api/core';
import {
  type Monitor,
  type Window,
  currentMonitor,
  getCurrentWindow,
} from '@tauri-apps/api/window';
import { TauriService } from './tauri.service';

export interface PixelSize {
  width: number;
  height: number;
}

export type Platform = 'linux' | 'macos' | 'windows';

/** Trailing debounce for the resize burst a drag or maximize produces. */
const RESIZE_SETTLE_MS = 150;

/**
 * First guess only. Every webview we ship (WebKitGTK, WKWebView,
 * WebView2) puts the OS in the user agent, but the UA is configurable
 * and subject to reduction, so the `host_os` command overrides it.
 */
export function detectPlatform(userAgent: string): Platform {
  if (/Macintosh|Mac OS X/i.test(userAgent)) return 'macos';
  if (/Windows/i.test(userAgent)) return 'windows';
  return 'linux';
}

/** Normalises `std::env::consts::OS` to the three we style for. */
export function platformFromOs(os: string): Platform {
  if (os === 'macos') return 'macos';
  if (os === 'windows') return 'windows';
  return 'linux';
}

/**
 * A window fills its monitor when it is fullscreen. tao's Linux
 * `is_fullscreen` only reflects our own `set_fullscreen` calls, never a
 * WM-initiated change, so on Linux the size is the ground truth.
 */
export function coversMonitor(size: PixelSize, monitor: PixelSize | null): boolean {
  if (!monitor) return false;
  return size.width >= monitor.width && size.height >= monitor.height;
}

/**
 * The window is frameless (`decorations: false`) on Linux and Windows,
 * so the UI draws its own title bar and caption buttons (minimize,
 * maximize, close, on the right). macOS keeps native decorations in
 * overlay mode: the real traffic lights float over our toolbar. Edge
 * resizing is native everywhere (Tauri's runtime installs an
 * undecorated-resize handler on Linux and Windows).
 *
 * Everything here is inert outside the Tauri webview so `ng serve` in
 * a browser still renders.
 */
@Injectable({ providedIn: 'root' })
export class WindowService {
  private readonly tauri = inject(TauriService);

  /** False under plain `ng serve` in a browser; every control hides. */
  readonly available: boolean = isTauri();

  readonly platform = signal<Platform>(detectPlatform(globalThis.navigator?.userAgent ?? ''));

  readonly maximized = signal(false);
  readonly fullscreen = signal(false);

  /** We draw close/minimize/zoom ourselves everywhere but macOS. */
  readonly customControls = computed(this.#computeCustomControls.bind(this));

  /** macOS overlays its own traffic lights on our toolbar; leave room for them. */
  readonly nativeTrafficLights = computed(this.#computeNativeTrafficLights.bind(this));

  /**
   * Linux gives a frameless window no compositor border, so the shell
   * draws a hairline while the window has a visible edge at all.
   */
  readonly hairline = computed(this.#computeHairline.bind(this));

  private readonly win: Window | null = this.available ? getCurrentWindow() : null;

  /** Monotonic token so a slow, superseded state read cannot overwrite a newer one. */
  private refreshSeq = 0;
  private resizeTimer: ReturnType<typeof setTimeout> | null = null;
  private toggling = false;

  constructor() {
    if (!this.win) return;
    void this.tauri
      .invoke<string>('host_os')
      .then((os) => this.platform.set(platformFromOs(os)))
      .catch(() => {
        // Keep the user-agent guess.
      });
    void this.refreshState();
    void this.win.onResized(() => this.scheduleRefresh());
  }

  async minimize(): Promise<void> {
    await this.win?.minimize();
  }

  /**
   * The WM applies the change asynchronously and reports it back via
   * a resize event, which is what refreshes the signals; reading back
   * immediately would only return the pre-toggle state.
   */
  async toggleMaximize(): Promise<void> {
    await this.win?.toggleMaximize();
  }

  async toggleFullscreen(): Promise<void> {
    if (!this.win || this.toggling) return;
    this.toggling = true;
    try {
      await this.win.setFullscreen(!this.fullscreen());
    } finally {
      this.toggling = false;
    }
  }

  async close(): Promise<void> {
    await this.win?.close();
  }

  #computeCustomControls(): boolean {
    return this.available && this.platform() !== 'macos';
  }

  #computeNativeTrafficLights(): boolean {
    return this.available && this.platform() === 'macos';
  }

  #computeHairline(): boolean {
    return this.available && this.platform() === 'linux' && !this.maximized() && !this.fullscreen();
  }

  private scheduleRefresh(): void {
    if (this.resizeTimer !== null) clearTimeout(this.resizeTimer);
    this.resizeTimer = setTimeout(() => {
      this.resizeTimer = null;
      void this.refreshState();
    }, RESIZE_SETTLE_MS);
  }

  private async refreshState(): Promise<void> {
    if (!this.win) return;
    this.refreshSeq += 1;
    const seq = this.refreshSeq;
    const [maximized, reported, size, monitor] = await Promise.all([
      this.win.isMaximized(),
      this.win.isFullscreen(),
      this.win.innerSize(),
      currentMonitor(),
    ]);
    if (seq !== this.refreshSeq) return;
    this.maximized.set(maximized);
    this.fullscreen.set(this.resolveFullscreen(reported, maximized, size, monitor));
  }

  /**
   * On Linux `is_fullscreen` is a cache of our own calls: it misses a
   * WM-initiated enter and stays true after a WM-initiated exit. So it
   * counts only while the window really fills the monitor, and a
   * monitor-filling window that is not maximized is fullscreen even if
   * the cache never heard about it.
   */
  private resolveFullscreen(
    reported: boolean,
    maximized: boolean,
    size: PixelSize,
    monitor: Monitor | null,
  ): boolean {
    if (this.platform() !== 'linux') return reported;
    const covers = coversMonitor(size, monitor?.size ?? null);
    return covers && (reported || !maximized);
  }
}
