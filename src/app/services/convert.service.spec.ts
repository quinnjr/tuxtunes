import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { ConvertService, DEFAULT_CONVERT_PREFS } from './convert.service';

interface ConvertPrefsLike {
  flac: { compression_level: number };
}
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

const started = (generation: number, total: number) => ({ generation, total });
const complete = (generation: number, over: Partial<Record<string, unknown>> = {}) => ({
  generation,
  total: 1,
  converted: 1,
  failed: 0,
  added_to_library: 0,
  cancelled: false,
  format: 'flac',
  ...over,
});

describe('ConvertService', () => {
  it('starts idle and reports running from the started event until the complete event', async () => {
    const { svc, ready, emit } = build();
    await ready;
    expect(svc.running()).toBe(false);

    emit('fs:convert-started', started(1, 2));
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

    emit('fs:convert-complete', complete(1, { total: 2, converted: 2, added_to_library: 2 }));
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
    emit('fs:convert-started', started(1, 9));
    emit('fs:convert-progress', { current: 0, total: 9, title: 'A', percent: 5 });

    await svc.cancel();

    expect(invoke).toHaveBeenCalledWith('cancel_convert');
    // Still running: only the worker's own complete event ends a batch,
    // so a cancel that loses a race cannot leave the UI lying.
    expect(svc.running()).toBe(true);

    emit('fs:convert-complete', {
      generation: 1,
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

  it('convert() only queues; the started event of a new run clears the previous record', async () => {
    const { svc, invoke, ready, emit } = build();
    await ready;
    emit('fs:convert-started', started(1, 1));
    emit('fs:convert-failed', { track_id: 1, title: 'Old', error: 'x' });
    emit('fs:convert-complete', complete(1, { converted: 0, failed: 1, format: 'm4a' }));

    await svc.convert([4, 5], 'flac');
    expect(invoke).toHaveBeenCalledWith('convert_tracks', {
      args: { track_ids: [4, 5], format: 'flac' },
    });
    // Nothing has changed yet: the worker has not picked the batch up.
    expect(svc.lastComplete()?.failed).toBe(1);
    expect(svc.failures()).toHaveLength(1);

    emit('fs:convert-started', started(2, 2));
    expect(svc.lastComplete()).toBeNull();
    expect(svc.failures()).toEqual([]);
    expect(svc.running()).toBe(true);
  });

  it('a batch that completes before the invoke resolves still nets out to idle', async () => {
    // The worker's events travel a different pipe from the invoke reply
    // and can land first; counting from the events alone keeps order.
    let release: () => void = () => {};
    const { svc, ready, emit } = build(
      (cmd) =>
        new Promise((resolve) => {
          if (cmd === 'convert_tracks') release = () => resolve(undefined);
          else resolve(undefined);
        }),
    );
    await ready;
    const queued = svc.convert([1], 'flac');
    emit('fs:convert-started', started(1, 1));
    emit('fs:convert-failed', { track_id: 1, title: 'Gone', error: 'missing' });
    emit('fs:convert-complete', complete(1, { converted: 0, failed: 1 }));
    release();
    await queued;

    expect(svc.running()).toBe(false);
    expect(svc.failures()).toHaveLength(1);
    expect(svc.lastComplete()?.failed).toBe(1);
  });

  it('a rejected queue changes nothing', async () => {
    const { svc, ready, emit } = build(async (cmd) => {
      if (cmd === 'convert_tracks') throw new Error('ffmpeg was not found on PATH');
    });
    await ready;
    emit('fs:convert-started', started(1, 1));
    emit('fs:convert-failed', { track_id: 1, title: 'Old', error: 'x' });
    emit('fs:convert-complete', complete(1, { converted: 0, failed: 1 }));

    await expect(svc.convert([1], 'flac')).rejects.toThrow('ffmpeg');

    expect(svc.running()).toBe(false);
    expect(svc.failures()).toHaveLength(1);
  });

  it('two batches queued back to back stay running until both complete and sum their tally', async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('fs:convert-started', started(1, 5));
    emit('fs:convert-started', started(2, 3));
    expect(svc.live().size).toBe(2);

    // First batch cancelled after two files; the second, already
    // cancelled too, reports zero work of its own.
    emit(
      'fs:convert-complete',
      complete(1, { total: 5, converted: 2, added_to_library: 2, cancelled: true }),
    );
    expect(svc.running()).toBe(true);
    emit('fs:convert-complete', complete(2, { total: 3, converted: 0, cancelled: true }));
    expect(svc.running()).toBe(false);
    expect(svc.lastComplete()).toEqual({
      generation: 2,
      total: 8,
      converted: 2,
      failed: 0,
      addedToLibrary: 2,
      cancelled: true,
      format: 'flac',
    });
    expect(svc.summary()).toBe(
      '2 converted to FLAC, 2 added to the library, cancelled with 6 left',
    );
  });

  it("a second batch started mid-run keeps the first batch's failures", async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('fs:convert-started', started(1, 1));
    emit('fs:convert-failed', { track_id: 1, title: 'Bad', error: 'x' });
    emit('fs:convert-started', started(2, 1));
    expect(svc.failures()).toHaveLength(1);

    emit('fs:convert-complete', complete(1, { converted: 0, failed: 1 }));
    emit('fs:convert-complete', complete(2, { format: 'm4a' }));
    expect(svc.lastComplete()?.format).toBe('flac+m4a');
    expect(svc.lastComplete()?.failed).toBe(1);
    expect(svc.summary()).toBe('1 converted, 1 failed');
  });

  it('serialises saves so the last reply is the last write', async () => {
    const order: number[] = [];
    let resolveFirst: (v: unknown) => void = () => {};
    const { svc, ready } = build((cmd, args) => {
      if (cmd !== 'set_convert_prefs') return Promise.resolve(undefined);
      const prefs = (args?.['prefs'] as ConvertPrefsLike).flac.compression_level;
      order.push(prefs);
      return prefs === 1
        ? new Promise((resolve) => {
            resolveFirst = resolve;
          })
        : Promise.resolve(args?.['prefs']);
    });
    await ready;
    const a = svc.savePrefs({
      ...DEFAULT_CONVERT_PREFS,
      flac: { ...DEFAULT_CONVERT_PREFS.flac, compression_level: 1 },
    });
    const b = svc.savePrefs({
      ...DEFAULT_CONVERT_PREFS,
      flac: { ...DEFAULT_CONVERT_PREFS.flac, compression_level: 2 },
    });
    for (let i = 0; i < 10; i += 1) await Promise.resolve();
    // The second save has not even been sent while the first is in flight.
    expect(order).toEqual([1]);
    resolveFirst({
      ...DEFAULT_CONVERT_PREFS,
      flac: { ...DEFAULT_CONVERT_PREFS.flac, compression_level: 1 },
    });
    await Promise.all([a, b]);
    expect(order).toEqual([1, 2]);
    expect(svc.prefs().flac.compression_level).toBe(2);
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
