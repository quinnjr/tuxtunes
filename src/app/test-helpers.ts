// Shared component-test helpers. Component fixtures all share the
// same set of dependencies (TauriService, the various app services),
// so we centralize the stub factory to keep specs focused on behavior
// rather than wiring.

import { Provider } from '@angular/core';
import { vi } from 'vitest';
import { DEFAULT_CONVERT_PREFS } from './services/convert.service';
import { DeviceService } from './services/device.service';
import { LibraryService } from './services/library.service';
import { PlaybackService } from './services/playback.service';
import { PreferencesService } from './services/preferences.service';
import { SyncService } from './services/sync.service';
import { TauriService } from './services/tauri.service';
import { UiService } from './services/ui.service';
import { ContextMenuService } from './services/context-menu.service';

export interface TauriStub {
  invoke: ReturnType<typeof vi.fn>;
  listen: ReturnType<typeof vi.fn>;
  emit(event: string, payload: unknown): void;
}

/**
 * Default invoke handler returning empty arrays for list-like commands
 * and undefined for everything else. Component ngOnInit hooks call
 * refreshTracks / refreshAlbums / etc. on mount, and those services
 * call `.map()` on the result; bare `undefined` would crash.
 */
export const defaultInvoke = async (cmd: string): Promise<unknown> => {
  if (
    cmd === 'list_tracks' ||
    cmd === 'list_albums' ||
    cmd === 'list_artists' ||
    cmd === 'tracks_for_album' ||
    cmd === 'list_playlists' ||
    cmd === 'open_playlist' ||
    cmd === 'list_sync_sources' ||
    cmd === 'list_audio_devices' ||
    cmd === 'get_distinct' ||
    cmd === 'list_devices' ||
    cmd === 'refresh_devices'
  ) {
    return [];
  }
  if (cmd === 'resolve_album_artwork' || cmd === 'resolve_track_artwork') {
    return null;
  }
  if (cmd === 'convert_available') {
    return true;
  }
  if (cmd === 'get_convert_prefs' || cmd === 'set_convert_prefs') {
    return DEFAULT_CONVERT_PREFS;
  }
  if (cmd === 'get_new_releases') {
    return {
      checked_at: null,
      running: false,
      started: false,
      window_days: 90,
      fetched_window_days: null,
      releases: [],
      last_error: null,
      qobuz_error: null,
    };
  }
  if (cmd === 'get_release_prefs' || cmd === 'set_release_prefs') {
    return { window_days: 90, qobuz_app_id: null };
  }
  if (cmd === 'get_audio_prefs') {
    return { device_id: null, exclusive: false, replaygain_mode: 'off' };
  }
  return undefined;
};

export interface TauriBridge {
  convertFileSrc: (path: string, protocol?: string) => string;
  invoke: ReturnType<typeof vi.fn>;
  transformCallback: ReturnType<typeof vi.fn>;
}

/**
 * Install the webview bridge `@tauri-apps/api` reads at call time.
 *
 * Specs that render a component importing `convertFileSrc` directly used
 * `vi.mock('@tauri-apps/api/core')` for this, but module mocks share a
 * registry and can leak into `tauri.service.spec`, which deliberately drives
 * the real service and installs its own bridge. Stubbing the bridge keeps
 * those two approaches from racing.
 */
export function installTauriBridge(
  convertFileSrc: (path: string) => string = (path) => `asset://${path}`,
): TauriBridge {
  const bridge: TauriBridge = {
    convertFileSrc: (path) => convertFileSrc(path),
    invoke: vi.fn(async () => undefined),
    transformCallback: vi.fn(() => 0),
  };
  (globalThis as { __TAURI_INTERNALS__?: TauriBridge }).__TAURI_INTERNALS__ = bridge;
  return bridge;
}

export function removeTauriBridge(): void {
  delete (globalThis as { __TAURI_INTERNALS__?: TauriBridge }).__TAURI_INTERNALS__;
}

export function tauriStub(
  invokeImpl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> = defaultInvoke,
): TauriStub {
  const listeners = new Map<string, ((p: unknown) => void)[]>();
  const invoke = vi.fn(invokeImpl as never);
  const listen = vi.fn(async (event: string, h: (p: unknown) => void) => {
    listeners.set(event, [...(listeners.get(event) ?? []), h]);
    return () => {
      listeners.set(
        event,
        (listeners.get(event) ?? []).filter((x) => x !== h),
      );
    };
  });
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  return { invoke, listen, emit };
}

/** Provider list to feed TestBed.configureTestingModule for components. */
export function appProviders(stub: TauriStub): Provider[] {
  return [
    { provide: TauriService, useValue: stub },
    DeviceService,
    LibraryService,
    PlaybackService,
    PreferencesService,
    SyncService,
    UiService,
    ContextMenuService,
  ];
}
