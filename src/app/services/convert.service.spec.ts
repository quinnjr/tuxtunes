import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { ConvertService, DEFAULT_CONVERT_PREFS } from './convert.service';
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
      { provide: ConvertService, useClass: ConvertService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(ConvertService));
  const ready = (async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  })();
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  return { svc, invoke, ready, emit };
}

describe('ConvertService', () => {
  it('starts idle and reports running from queueing until the complete event', async () => {
    const { svc, ready, emit } = build();
    await ready;
    expect(svc.running()).toBe(false);

    await svc.convert([1, 2], 'flac');
    expect(svc.running()).toBe(true);
    emit('fs:convert-progress', {
      current: 0,
      total: 2,
      title: 'Song',
      percent: 40,
    });
    expect(svc.running()).toBe(true);
    expect(svc.progress()).toEqual({
      current: 0,
      total: 2,
      title: 'Song',
      percent: 40,
    });

    emit('fs:convert-complete', {
      total: 2,
      converted: 2,
      failed: 0,
      added_to_library: 2,
      cancelled: false,
      format: 'flac',
    });
    expect(svc.running()).toBe(false);
    expect(svc.lastComplete()?.converted).toBe(2);
    expect(svc.lastComplete()?.addedToLibrary).toBe(2);
    expect(svc.lastComplete()?.cancelled).toBe(false);
  });

  it('carries a null percent through for a track of unknown duration', async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('fs:convert-progress', {
      current: 0,
      total: 1,
      title: 'Song',
      percent: null,
    });
    expect(svc.progress()?.percent).toBeNull();
  });

  it('cancel() invokes the backend and leaves the complete event to clear the run', async () => {
    const { svc, invoke, ready, emit } = build();
    await ready;
    await svc.convert([1], 'flac');
    emit('fs:convert-progress', { current: 0, total: 9, title: 'A', percent: 5 });

    await svc.cancel();

    expect(invoke).toHaveBeenCalledWith('cancel_convert');
    // Still running: only the worker's own complete event ends a batch,
    // so a cancel that loses a race cannot leave the UI lying.
    expect(svc.running()).toBe(true);

    emit('fs:convert-complete', {
      total: 9,
      converted: 1,
      failed: 0,
      added_to_library: 1,
      cancelled: true,
      format: 'flac',
    });
    expect(svc.running()).toBe(false);
    expect(svc.lastComplete()?.cancelled).toBe(true);
  });

  it('collects per-file failures', async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('fs:convert-failed', { track_id: 3, title: 'Bad', error: 'ffmpeg failed' });
    expect(svc.failures()).toEqual([{ trackId: 3, title: 'Bad', error: 'ffmpeg failed' }]);
  });

  it('convert() clears the previous batch and passes snake_case args', async () => {
    const { svc, invoke, ready, emit } = build();
    await ready;
    emit('fs:convert-complete', {
      total: 1,
      converted: 1,
      failed: 0,
      added_to_library: 0,
      cancelled: false,
      format: 'm4a',
    });
    emit('fs:convert-failed', { track_id: 1, title: 'Old', error: 'x' });

    await svc.convert([4, 5], 'flac');

    expect(svc.lastComplete()).toBeNull();
    expect(svc.failures()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith('convert_tracks', {
      args: { track_ids: [4, 5], format: 'flac' },
    });
  });

  it('is running from the moment a batch is queued, before any progress arrives', async () => {
    const { svc, ready } = build();
    await ready;
    await svc.convert([1], 'm4a');
    expect(svc.running()).toBe(true);
    expect(svc.progress()).toBeNull();
  });

  it('a rejected queue leaves nothing running and no stale clear', async () => {
    const { svc, ready, emit } = build(async (cmd) => {
      if (cmd === 'convert_tracks') throw new Error('convert worker has exited');
    });
    await ready;
    emit('fs:convert-failed', { track_id: 1, title: 'Old', error: 'x' });

    await expect(svc.convert([1], 'flac')).rejects.toThrow('worker has exited');

    expect(svc.running()).toBe(false);
    // The previous run's record is kept: no batch replaced it.
    expect(svc.failures()).toHaveLength(1);
  });

  it('two batches queued back to back stay running until both complete and sum their tally', async () => {
    const { svc, ready, emit } = build();
    await ready;
    await svc.convert([1, 2, 3, 4, 5], 'flac');
    await svc.convert([6, 7, 8], 'flac');
    expect(svc.pending()).toBe(2);

    // First batch cancelled after two files; the second, already
    // cancelled too, reports zero work of its own.
    emit('fs:convert-complete', {
      total: 5,
      converted: 2,
      failed: 0,
      added_to_library: 2,
      cancelled: true,
      format: 'flac',
    });
    expect(svc.running()).toBe(true);
    emit('fs:convert-complete', {
      total: 3,
      converted: 0,
      failed: 0,
      added_to_library: 0,
      cancelled: true,
      format: 'flac',
    });
    expect(svc.running()).toBe(false);
    expect(svc.lastComplete()).toEqual({
      total: 8,
      converted: 2,
      failed: 0,
      addedToLibrary: 2,
      cancelled: true,
      format: 'flac',
    });
  });

  it("a second batch queued mid-run keeps the first batch's failures", async () => {
    const { svc, ready, emit } = build();
    await ready;
    await svc.convert([1], 'flac');
    emit('fs:convert-failed', { track_id: 1, title: 'Bad', error: 'x' });
    await svc.convert([2], 'm4a');
    expect(svc.failures()).toHaveLength(1);

    for (const format of ['flac', 'm4a']) {
      emit('fs:convert-complete', {
        total: 1,
        converted: format === 'm4a' ? 1 : 0,
        failed: format === 'flac' ? 1 : 0,
        added_to_library: 0,
        cancelled: false,
        format,
      });
    }
    expect(svc.lastComplete()?.format).toBe('flac+m4a');
    expect(svc.lastComplete()?.failed).toBe(1);
  });

  it('adopts the clamped prefs the backend returns rather than the draft sent', async () => {
    const clamped = {
      ...DEFAULT_CONVERT_PREFS,
      flac: { compression_level: 12, sample_rate: null, bit_depth: null },
    };
    const { svc, ready } = build(async (cmd) =>
      cmd === 'set_convert_prefs' ? clamped : undefined,
    );
    await ready;
    await svc.savePrefs({
      ...DEFAULT_CONVERT_PREFS,
      flac: { compression_level: 99, sample_rate: null, bit_depth: null },
    });
    expect(svc.prefs().flac.compression_level).toBe(12);
  });

  it('refresh() loads ffmpeg availability alongside the prefs', async () => {
    const { svc, ready } = build(async (cmd) => {
      if (cmd === 'convert_available') return false;
      if (cmd === 'get_convert_prefs') return DEFAULT_CONVERT_PREFS;
      return undefined;
    });
    await ready;
    await svc.refresh();
    expect(svc.available()).toBe(false);
    expect(svc.prefs()).toEqual(DEFAULT_CONVERT_PREFS);
  });
});
