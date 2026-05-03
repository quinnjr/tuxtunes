import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import type { ConflictRules } from '../models/sync';
import { SyncService } from './sync.service';
import { TauriService } from './tauri.service';

type Listener = (payload: unknown) => void;

function build(
  invokeImpl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> = async () => {},
) {
  const listeners = new Map<string, Listener[]>();
  const invoke = vi.fn(invokeImpl as never);
  const stubTauri = {
    invoke,
    listen: vi.fn(async (event: string, h: Listener) => {
      listeners.set(event, [...(listeners.get(event) ?? []), h]);
      return () => {};
    }),
  } as unknown as TauriService;
  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: SyncService, useClass: SyncService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(SyncService));
  const ready = (async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  })();
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  return { svc, invoke, ready, emit };
}

const RAW_SOURCE = {
  id: 1,
  name: 'X',
  kind: 'itunes_itl',
  source_path: '/x.itl',
  last_sync_at: null,
  last_sync_hash: null,
  path_mappings: [],
  conflict_rules: {
    rating: 'prefer_source',
    play_count: 'prefer_source',
    skip_count: 'prefer_source',
    last_played: 'prefer_source',
    last_skipped: 'prefer_source',
    loved: 'prefer_source',
    deletes: 'respect',
  } as ConflictRules,
  auto_copy_files: true,
};

describe('SyncService', () => {
  it('runState computed reflects progress / error / idle transitions', () => {
    const { svc } = build();
    expect(svc.runState()).toBe('idle');
    svc.progress.set({
      sourceId: 1,
      phase: 'decoding',
      current: 0,
      total: 100,
      message: '',
    });
    expect(svc.runState()).toBe('running');
    svc.lastError.set({ sourceId: 1, error: 'boom' });
    expect(svc.runState()).toBe('error');
  });

  it('refreshSources() maps snake_case payload', async () => {
    const { svc } = build(async () => [RAW_SOURCE]);
    await svc.refreshSources();
    expect(svc.sources()).toHaveLength(1);
    expect(svc.sources()[0].sourcePath).toBe('/x.itl');
    expect(svc.sources()[0].autoCopyFiles).toBe(true);
  });

  it('addSource() forwards args and refreshes the list', async () => {
    let calls = 0;
    const { svc, invoke } = build(async (cmd) => {
      calls += 1;
      if (cmd === 'add_sync_source') return 7;
      if (cmd === 'list_sync_sources') return [RAW_SOURCE];
      return;
    });
    const id = await svc.addSource({
      name: 'X',
      source_path: '/x.itl',
      path_mappings: [],
      conflict_rules: RAW_SOURCE.conflict_rules,
      auto_copy_files: true,
    });
    expect(id).toBe(7);
    expect(calls).toBe(2);
    expect(invoke).toHaveBeenCalledWith('add_sync_source', expect.any(Object));
  });

  it('runNow() resets progress/warnings/last-complete/last-error and forwards', async () => {
    const { svc, invoke } = build();
    svc.progress.set({
      sourceId: 1,
      phase: 'decoding',
      current: 0,
      total: 0,
      message: '',
    });
    svc.warnings.set([{ sourceId: 1, kind: 'missing_source_file', detail: 'x' }]);
    svc.lastComplete.set({
      sourceId: 1,
      insertedTracks: 0,
      updatedTracks: 0,
      deletedTracks: 0,
      insertedPlaylists: 0,
      updatedPlaylists: 0,
      deletedPlaylists: 0,
    });
    svc.lastError.set({ sourceId: 1, error: 'old' });
    await svc.runNow(1);
    expect(svc.progress()).toBeNull();
    expect(svc.warnings()).toEqual([]);
    expect(svc.lastComplete()).toBeNull();
    expect(svc.lastError()).toBeNull();
    expect(invoke).toHaveBeenCalledWith('run_sync_now', { sourceId: 1 });
  });

  it('listens for sync events and updates signals', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('sync:progress', {
      source_id: 1,
      phase: 'decoding',
      current: 5,
      total: 10,
      message: 'reading',
    });
    expect(harness.svc.progress()?.current).toBe(5);
    expect(harness.svc.progress()?.message).toBe('reading');

    harness.emit('sync:warning', {
      source_id: 1,
      kind: 'missing_source_file',
      detail: 'gone',
    });
    expect(harness.svc.warnings()).toHaveLength(1);

    harness.emit('sync:complete', {
      source_id: 1,
      inserted_tracks: 3,
      updated_tracks: 2,
      deleted_tracks: 1,
      inserted_playlists: 0,
      updated_playlists: 0,
      deleted_playlists: 0,
    });
    expect(harness.svc.lastComplete()?.insertedTracks).toBe(3);

    harness.emit('sync:failed', { source_id: 1, error: 'boom' });
    expect(harness.svc.lastError()?.error).toBe('boom');
  });

  it('warnings cap at 50 entries (slice -49 + new)', async () => {
    const harness = build();
    await harness.ready;
    for (let i = 0; i < 100; i += 1) {
      harness.emit('sync:warning', {
        source_id: 1,
        kind: 'missing_source_file',
        detail: `n${i}`,
      });
    }
    expect(harness.svc.warnings()).toHaveLength(50);
    // The most recent warning should be at the end of the buffer.
    expect(harness.svc.warnings().at(-1)?.detail).toBe('n99');
  });

  it('ngOnDestroy() invokes every captured unlistener', async () => {
    // Capture the unlisten functions the service receives via a custom
    // listen() impl. Verify each one is called exactly once on destroy.
    const unlistenSpies: ReturnType<typeof vi.fn>[] = [];
    const stubTauri = {
      invoke: vi.fn(async () => {}),
      listen: vi.fn(async () => {
        const u = vi.fn();
        unlistenSpies.push(u);
        return u;
      }),
    } as unknown as TauriService;
    const injector = Injector.create({
      providers: [
        { provide: TauriService, useValue: stubTauri },
        { provide: SyncService, useClass: SyncService },
      ],
    });
    const svc = runInInjectionContext(injector, () => injector.get(SyncService));
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
    expect(unlistenSpies.length).toBeGreaterThan(0);
    svc.ngOnDestroy();
    for (const u of unlistenSpies) expect(u).toHaveBeenCalledTimes(1);
    // Calling again should be safe — the unlistener array is cleared.
    svc.ngOnDestroy();
    for (const u of unlistenSpies) expect(u).toHaveBeenCalledTimes(1);
  });
});
