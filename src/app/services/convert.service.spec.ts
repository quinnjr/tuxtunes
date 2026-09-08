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
  it('starts idle and reports running only between progress and complete', async () => {
    const { svc, ready, emit } = build();
    await ready;
    expect(svc.running()).toBe(false);

    emit('fs:convert-progress', { current: 0, total: 2, track_id: 7, title: 'Song' });
    expect(svc.running()).toBe(true);
    expect(svc.progress()).toEqual({ current: 0, total: 2, trackId: 7, title: 'Song' });

    emit('fs:convert-complete', { total: 2, converted: 2, failed: 0, format: 'flac' });
    expect(svc.running()).toBe(false);
    expect(svc.lastComplete()?.converted).toBe(2);
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
    emit('fs:convert-complete', { total: 1, converted: 1, failed: 0, format: 'm4a' });
    emit('fs:convert-failed', { track_id: 1, title: 'Old', error: 'x' });

    await svc.convert([4, 5], 'flac');

    expect(svc.lastComplete()).toBeNull();
    expect(svc.failures()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith('convert_tracks', {
      args: { track_ids: [4, 5], format: 'flac' },
    });
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
