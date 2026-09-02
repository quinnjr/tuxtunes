import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WindowService, detectPlatform } from './window.service';

/**
 * Drives the real `@tauri-apps/api/window` through a stubbed
 * `__TAURI_INTERNALS__` bridge, the same approach as tauri.service.spec:
 * no module mocks to race, and the genuine `Window` methods run.
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

const g = globalThis as unknown as GlobalWithTauri;
let internals: Internals;
let state: { maximized: boolean; fullscreen: boolean };

function setUserAgent(ua: string): void {
  Object.defineProperty(g.navigator, 'userAgent', { value: ua, configurable: true });
}

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

beforeEach(() => {
  state = { maximized: false, fullscreen: false };
  internals = {
    transformCallback: vi.fn(() => 1),
    invoke: vi.fn(async (cmd: string) => {
      if (cmd === 'plugin:window|is_maximized') return state.maximized;
      if (cmd === 'plugin:window|is_fullscreen') return state.fullscreen;
      if (cmd === 'plugin:window|toggle_maximize') state.maximized = !state.maximized;
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
  delete g.__TAURI_INTERNALS__;
  delete g.__TAURI_EVENT_PLUGIN_INTERNALS__;
  delete g.isTauri;
});

const calls = () => internals.invoke.mock.calls.map((c) => c[0] as string);

describe('detectPlatform', () => {
  it('recognises the three webview user agents', () => {
    expect(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit')).toBe(
      'macos',
    );
    expect(detectPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/120')).toBe('windows');
    expect(detectPlatform('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit')).toBe('linux');
    expect(detectPlatform('')).toBe('linux');
  });
});

describe('WindowService', () => {
  it('draws custom controls and resize edges on Linux', () => {
    const svc = new WindowService();
    expect(svc.platform).toBe('linux');
    expect(svc.customControls).toBe(true);
    expect(svc.customResize).toBe(true);
  });

  it('draws custom controls but no resize edges on Windows', () => {
    setUserAgent('Mozilla/5.0 (Windows NT 10.0; Win64; x64)');
    const svc = new WindowService();
    expect(svc.customControls).toBe(true);
    expect(svc.customResize).toBe(false);
  });

  it('leaves everything native on macOS', () => {
    setUserAgent('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)');
    const svc = new WindowService();
    expect(svc.customControls).toBe(false);
    expect(svc.customResize).toBe(false);
  });

  it('is inert outside the Tauri webview', async () => {
    delete g.isTauri;
    const svc = new WindowService();
    expect(svc.available).toBe(false);
    expect(svc.customControls).toBe(false);
    await svc.startDragging();
    await svc.close();
    expect(internals.invoke).not.toHaveBeenCalled();
  });

  it('reads maximized/fullscreen state on construction', async () => {
    state.maximized = true;
    const svc = new WindowService();
    await settle();
    expect(svc.maximized()).toBe(true);
    expect(svc.fullscreen()).toBe(false);
    expect(calls()).toContain('plugin:window|is_maximized');
    expect(calls()).toContain('plugin:event|listen');
  });

  it('forwards the window actions to the window plugin with the current label', async () => {
    const svc = new WindowService();
    await svc.startDragging();
    await svc.minimize();
    await svc.close();
    await svc.startResizeDragging('SouthEast');
    const named = (cmd: string): unknown =>
      internals.invoke.mock.calls.find((c) => c[0] === cmd)?.[1];
    expect(named('plugin:window|start_dragging')).toEqual({ label: 'main' });
    expect(named('plugin:window|minimize')).toEqual({ label: 'main' });
    expect(named('plugin:window|close')).toEqual({ label: 'main' });
    expect(named('plugin:window|start_resize_dragging')).toEqual({
      label: 'main',
      value: 'SouthEast',
    });
  });

  it('toggleMaximize() refreshes the maximized signal', async () => {
    const svc = new WindowService();
    await settle();
    expect(svc.maximized()).toBe(false);
    await svc.toggleMaximize();
    expect(svc.maximized()).toBe(true);
  });

  it('toggleFullscreen() flips against the current signal value', async () => {
    const svc = new WindowService();
    await settle();
    await svc.toggleFullscreen();
    const set = internals.invoke.mock.calls.find((c) => c[0] === 'plugin:window|set_fullscreen');
    expect(set?.[1]).toEqual({ label: 'main', value: true });
  });
});
