import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WindowService, coversMonitor, detectPlatform, platformFromOs } from './window.service';

/**
 * Drives the real `@tauri-apps/api/window` through a stubbed
 * `__TAURI_INTERNALS__` bridge, the same approach as tauri.service.spec:
 * no module mocks to race, and the genuine `Window` methods run.
 *
 * The stub models the runtime honestly: `toggle_maximize` and
 * `set_fullscreen` return before the window changes, then the change
 * lands on a later tick and is announced through `tauri://resize`,
 * because that is the only way the service learns about it.
 */
interface Internals {
  invoke: ReturnType<typeof vi.fn>;
  transformCallback: ReturnType<typeof vi.fn>;
  metadata: { currentWindow: { label: string }; currentWebview: { label: string } };
}

interface GlobalWithTauri {
  __TAURI_INTERNALS__?: Internals;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: ReturnType<typeof vi.fn> };
  isTauri?: boolean;
  navigator: Navigator;
}

interface Size {
  width: number;
  height: number;
}

const MONITOR: Size = { width: 2560, height: 1440 };
const NORMAL: Size = { width: 1200, height: 800 };
/** Maximized on a desktop with a panel: full width, not full height. */
const MAXIMIZED: Size = { width: 2560, height: 1400 };

const g = globalThis as unknown as GlobalWithTauri;
let internals: Internals;
let state: { maximized: boolean; fullscreenCache: boolean; size: Size; os: string };
let callbacks: Map<number, (payload: unknown) => void>;

/** The wire shape `mapMonitor` in @tauri-apps/api/window expects. */
function monitorPayload() {
  return {
    name: 'DP-1',
    scaleFactor: 1,
    position: { x: 0, y: 0 },
    size: MONITOR,
    workArea: { position: { x: 0, y: 0 }, size: MAXIMIZED },
  };
}

function setUserAgent(ua: string): void {
  Object.defineProperty(g.navigator, 'userAgent', { value: ua, configurable: true });
}

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

/** The WM applied a change: update the model and announce a resize. */
async function windowChanged(patch: Partial<typeof state>): Promise<void> {
  Object.assign(state, patch);
  for (const cb of callbacks.values()) {
    cb({ event: 'tauri://resize', id: 1, payload: state.size });
  }
  await vi.advanceTimersByTimeAsync(200);
  await settle();
}

function create(): WindowService {
  return TestBed.inject(WindowService);
}

beforeEach(() => {
  vi.useFakeTimers();
  callbacks = new Map();
  state = { maximized: false, fullscreenCache: false, size: NORMAL, os: 'linux' };
  let nextId = 1;
  internals = {
    transformCallback: vi.fn((cb: (payload: unknown) => void) => {
      const id = nextId;
      nextId += 1;
      callbacks.set(id, cb);
      return id;
    }),
    invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'host_os') return state.os;
      if (cmd === 'plugin:window|is_maximized') return state.maximized;
      if (cmd === 'plugin:window|is_fullscreen') return state.fullscreenCache;
      if (cmd === 'plugin:window|inner_size') return state.size;
      if (cmd === 'plugin:window|current_monitor') return monitorPayload();
      if (cmd === 'plugin:window|set_fullscreen') {
        // tao writes its cache synchronously; the WM resizes later.
        state.fullscreenCache = Boolean(args?.['value']);
      }
      if (cmd === 'plugin:event|listen') return 1;
      return undefined;
    }),
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  };
  g.__TAURI_INTERNALS__ = internals;
  g.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: vi.fn() };
  g.isTauri = true;
  setUserAgent('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15');
});

afterEach(() => {
  vi.useRealTimers();
  delete g.__TAURI_INTERNALS__;
  delete g.__TAURI_EVENT_PLUGIN_INTERNALS__;
  delete g.isTauri;
});

const calls = () => internals.invoke.mock.calls.map((c) => c[0] as string);
const argsOf = (cmd: string): unknown => internals.invoke.mock.calls.find((c) => c[0] === cmd)?.[1];

describe('detectPlatform', () => {
  it('recognises the three webview user agents and defaults to linux', () => {
    expect(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit')).toBe(
      'macos',
    );
    expect(detectPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/120')).toBe('windows');
    expect(detectPlatform('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit')).toBe('linux');
    expect(detectPlatform('')).toBe('linux');
  });
});

describe('platformFromOs', () => {
  it('maps std::env::consts::OS tokens, treating the BSDs as linux', () => {
    expect(platformFromOs('macos')).toBe('macos');
    expect(platformFromOs('windows')).toBe('windows');
    expect(platformFromOs('linux')).toBe('linux');
    expect(platformFromOs('freebsd')).toBe('linux');
  });
});

describe('coversMonitor', () => {
  it('is true only when the window is at least the monitor size', () => {
    expect(coversMonitor(MONITOR, MONITOR)).toBe(true);
    expect(coversMonitor(MAXIMIZED, MONITOR)).toBe(false);
    expect(coversMonitor(MONITOR, null)).toBe(false);
  });
});

describe('WindowService', () => {
  it('draws custom controls on Linux and a hairline while windowed', async () => {
    const svc = create();
    await settle();
    expect(svc.platform()).toBe('linux');
    expect(svc.customControls()).toBe(true);
    expect(svc.nativeTrafficLights()).toBe(false);
    expect(svc.hairline()).toBe(true);
  });

  it('draws custom controls but no hairline on Windows', async () => {
    setUserAgent('Mozilla/5.0 (Windows NT 10.0; Win64; x64)');
    state.os = 'windows';
    const svc = create();
    await settle();
    expect(svc.customControls()).toBe(true);
    expect(svc.hairline()).toBe(false);
  });

  it('leaves everything native on macOS', async () => {
    setUserAgent('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)');
    state.os = 'macos';
    const svc = create();
    await settle();
    expect(svc.customControls()).toBe(false);
    expect(svc.nativeTrafficLights()).toBe(true);
    expect(svc.hairline()).toBe(false);
  });

  it('lets host_os override a misleading user agent', async () => {
    setUserAgent('TuxTunes/1.0');
    state.os = 'macos';
    const svc = create();
    expect(svc.platform()).toBe('linux');
    await settle();
    expect(svc.platform()).toBe('macos');
    expect(svc.customControls()).toBe(false);
  });

  it('is inert outside the Tauri webview', async () => {
    delete g.isTauri;
    const svc = create();
    expect(svc.available).toBe(false);
    expect(svc.customControls()).toBe(false);
    expect(svc.hairline()).toBe(false);
    await svc.toggleMaximize();
    await svc.toggleFullscreen();
    await svc.close();
    expect(internals.invoke).not.toHaveBeenCalled();
  });

  it('reads window state on construction and subscribes to resizes', async () => {
    state.maximized = true;
    state.size = MAXIMIZED;
    const svc = create();
    await settle();
    expect(svc.maximized()).toBe(true);
    expect(svc.fullscreen()).toBe(false);
    expect(svc.hairline()).toBe(false);
    expect(argsOf('plugin:event|listen')).toEqual(
      expect.objectContaining({ event: 'tauri://resize' }),
    );
  });

  it('forwards minimize and close to the window plugin with the current label', async () => {
    const svc = create();
    await svc.minimize();
    await svc.close();
    expect(argsOf('plugin:window|minimize')).toEqual({ label: 'main' });
    expect(argsOf('plugin:window|close')).toEqual({ label: 'main' });
  });

  it('toggleMaximize() learns the new state from the resize the WM sends back', async () => {
    const svc = create();
    await settle();
    await svc.toggleMaximize();
    expect(calls()).toContain('plugin:window|toggle_maximize');
    // Nothing has happened yet: the WM has not applied it.
    expect(svc.maximized()).toBe(false);
    await windowChanged({ maximized: true, size: MAXIMIZED });
    expect(svc.maximized()).toBe(true);
    expect(svc.hairline()).toBe(false);
  });

  it('coalesces a burst of resize events into one state read', async () => {
    const svc = create();
    await settle();
    internals.invoke.mockClear();
    for (const cb of callbacks.values()) {
      for (let i = 0; i < 20; i += 1) cb({ event: 'tauri://resize', id: 1, payload: NORMAL });
    }
    await vi.advanceTimersByTimeAsync(200);
    await settle();
    expect(calls().filter((c) => c === 'plugin:window|is_maximized')).toHaveLength(1);
    expect(svc.maximized()).toBe(false);
  });

  it('toggleFullscreen() enters, then exits, tracking the signal', async () => {
    const svc = create();
    await settle();
    await svc.toggleFullscreen();
    expect(argsOf('plugin:window|set_fullscreen')).toEqual({ label: 'main', value: true });
    await windowChanged({ size: MONITOR });
    expect(svc.fullscreen()).toBe(true);
    expect(svc.hairline()).toBe(false);

    internals.invoke.mockClear();
    await svc.toggleFullscreen();
    expect(argsOf('plugin:window|set_fullscreen')).toEqual({ label: 'main', value: false });
    await windowChanged({ size: NORMAL });
    expect(svc.fullscreen()).toBe(false);
  });

  it('on Linux, detects a WM-initiated fullscreen that the cache never saw', async () => {
    const svc = create();
    await settle();
    await windowChanged({ size: MONITOR });
    expect(svc.fullscreen()).toBe(true);
    // ...and the button now exits rather than re-entering.
    await svc.toggleFullscreen();
    expect(argsOf('plugin:window|set_fullscreen')).toEqual({ label: 'main', value: false });
  });

  it('on Linux, clears fullscreen after a WM-initiated exit leaves the cache stale', async () => {
    const svc = create();
    await settle();
    await svc.toggleFullscreen();
    await windowChanged({ size: MONITOR });
    expect(svc.fullscreen()).toBe(true);
    // The WM un-fullscreens; tao's cache still says true.
    await windowChanged({ size: NORMAL });
    expect(state.fullscreenCache).toBe(true);
    expect(svc.fullscreen()).toBe(false);
  });

  it('on Linux, a maximized window that happens to fill the monitor is not fullscreen', async () => {
    const svc = create();
    await settle();
    await windowChanged({ maximized: true, size: MONITOR });
    expect(svc.maximized()).toBe(true);
    expect(svc.fullscreen()).toBe(false);
  });

  it('off Linux, trusts is_fullscreen directly', async () => {
    state.os = 'windows';
    const svc = create();
    await settle();
    await windowChanged({ fullscreenCache: true, size: NORMAL });
    expect(svc.fullscreen()).toBe(true);
  });

  it('ignores a second toggleFullscreen() while the first is in flight', async () => {
    let release: () => void = () => undefined;
    internals.invoke.mockImplementationOnce(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    const svc = create();
    await settle();
    internals.invoke.mockClear();
    internals.invoke.mockImplementationOnce(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    const first = svc.toggleFullscreen();
    await svc.toggleFullscreen();
    release();
    await first;
    expect(calls().filter((c) => c === 'plugin:window|set_fullscreen')).toHaveLength(1);
  });

  it('a stale state read cannot overwrite a newer one', async () => {
    const svc = create();
    await settle();
    // First read: slow, returns the old (unmaximized) state.
    const gate: (() => void)[] = [];
    internals.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'plugin:window|is_maximized') {
        const snapshot = state.maximized;
        await new Promise<void>((resolve) => gate.push(resolve));
        return snapshot;
      }
      if (cmd === 'plugin:window|is_fullscreen') return state.fullscreenCache;
      if (cmd === 'plugin:window|inner_size') return state.size;
      if (cmd === 'plugin:window|current_monitor') return monitorPayload();
      return undefined;
    });
    for (const cb of callbacks.values()) cb({ event: 'tauri://resize', id: 1, payload: NORMAL });
    await vi.advanceTimersByTimeAsync(200);
    // Second read starts after the WM maximized.
    state.maximized = true;
    state.size = MAXIMIZED;
    for (const cb of callbacks.values()) cb({ event: 'tauri://resize', id: 1, payload: MAXIMIZED });
    await vi.advanceTimersByTimeAsync(200);
    expect(gate).toHaveLength(2);
    // Resolve the newer read first, then the stale one.
    gate[1]();
    await settle();
    expect(svc.maximized()).toBe(true);
    gate[0]();
    await settle();
    expect(svc.maximized()).toBe(true);
  });
});
