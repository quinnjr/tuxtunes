// Shared component-test helpers. Component fixtures all share the
// same set of dependencies (TauriService, the various app services),
// so we centralize the stub factory to keep specs focused on behavior
// rather than wiring.

import { Provider } from '@angular/core';
import { vi } from 'vitest';
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
    cmd === 'list_sync_sources' ||
    cmd === 'list_audio_devices' ||
    cmd === 'get_distinct'
  ) {
    return [];
  }
  if (cmd === 'get_audio_prefs') {
    return { device_id: null, exclusive: false, replaygain_mode: 'off' };
  }
  return undefined;
};

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
    LibraryService,
    PlaybackService,
    PreferencesService,
    SyncService,
    UiService,
    ContextMenuService,
  ];
}
