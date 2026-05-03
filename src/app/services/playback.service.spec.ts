import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { PlaybackService, type TrackRow } from './playback.service';
import { TauriService } from './tauri.service';

type Listener = (payload: unknown) => void;

interface Harness {
  svc: PlaybackService;
  invoke: ReturnType<typeof vi.fn>;
  /** Fire a listener registered for `event` with the given payload. */
  emit: (event: string, payload: unknown) => void;
  /** Wait for the constructor's subscribeEvents() promise to settle. */
  ready: Promise<void>;
}

function build(
  invokeImpl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> = async () => {},
): Harness {
  const listeners = new Map<string, Listener[]>();
  const invokeSpy = vi.fn(invokeImpl as never);
  const stubTauri = {
    invoke: invokeSpy,
    listen: vi.fn(async (event: string, handler: Listener) => {
      listeners.set(event, [...(listeners.get(event) ?? []), handler]);
      return () => {
        listeners.set(
          event,
          (listeners.get(event) ?? []).filter((h) => h !== handler),
        );
      };
    }),
  } as unknown as TauriService;

  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: PlaybackService, useClass: PlaybackService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(PlaybackService));

  // Wait for subscribeEvents() to complete by yielding. The constructor
  // schedules the listen() awaits as microtasks; one tick is enough
  // because each `await` is a resolved Promise from our stub.
  const ready = (async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  })();

  const emit: Harness['emit'] = (event, payload) => {
    for (const handler of listeners.get(event) ?? []) handler(payload);
  };

  return { svc, invoke: invokeSpy, emit, ready };
}

const TRACK: TrackRow = {
  id: 42,
  title: 'T',
  artist: 'A',
  album: 'Al',
  durationMs: 180_000,
  filePath: '/tmp/a.flac',
  sampleRate: 44_100,
  bitDepth: 16,
  kind: 'flac',
  playCount: 0,
  skipCount: 0,
};

describe('PlaybackService', () => {
  it('initializes signals to defaults', () => {
    const { svc } = build();
    expect(svc.currentTrackId()).toBeNull();
    expect(svc.state()).toBe('stopped');
    expect(svc.positionMs()).toBe(0);
    expect(svc.durationMs()).toBe(0);
    expect(svc.volume()).toBe(100);
    expect(svc.queue()).toEqual([]);
  });

  it('forwards play / pause / resume / stop / seek / setVolume to Tauri', async () => {
    const { svc, invoke } = build();
    await svc.play(7);
    await svc.pause();
    await svc.resume();
    await svc.stop();
    await svc.seek(1000);
    await svc.setVolume(50);
    expect(invoke).toHaveBeenCalledWith('play_track', { trackId: 7 });
    expect(invoke).toHaveBeenCalledWith('pause');
    expect(invoke).toHaveBeenCalledWith('resume');
    expect(invoke).toHaveBeenCalledWith('stop');
    expect(invoke).toHaveBeenCalledWith('seek', { positionMs: 1000 });
    expect(invoke).toHaveBeenCalledWith('set_volume', { volume: 50 });
  });

  it('togglePlay() pauses while playing, resumes while paused, no-ops otherwise', async () => {
    const { svc, invoke } = build();
    svc.state.set('playing');
    await svc.togglePlay();
    expect(invoke).toHaveBeenCalledWith('pause');
    invoke.mockClear();
    svc.state.set('paused');
    await svc.togglePlay();
    expect(invoke).toHaveBeenCalledWith('resume');
    invoke.mockClear();
    svc.state.set('stopped');
    await svc.togglePlay();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('queue helpers enqueue, play-next, remove, reorder, advance, clear', async () => {
    const { svc, invoke } = build();
    const a = { ...TRACK, id: 1 };
    const b = { ...TRACK, id: 2 };
    const c = { ...TRACK, id: 3 };
    svc.enqueue(a);
    svc.enqueue(b);
    svc.playNext(c); // c at the head.
    expect(svc.queue().map((t) => t.id)).toEqual([3, 1, 2]);

    svc.removeFromQueue(1); // drop a (index 1 = id 1).
    expect(svc.queue().map((t) => t.id)).toEqual([3, 2]);

    svc.reorderQueue(0, 1); // swap.
    expect(svc.queue().map((t) => t.id)).toEqual([2, 3]);

    const popped = await svc.advanceFromQueue();
    expect(popped?.id).toBe(2);
    expect(invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
    expect(svc.queue().map((t) => t.id)).toEqual([3]);

    svc.clearQueue();
    expect(svc.queue()).toEqual([]);
  });

  it('advanceFromQueue() returns null on empty queue', async () => {
    const { svc, invoke } = build();
    const out = await svc.advanceFromQueue();
    expect(out).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('listens for engine events and updates state signals', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('playback:track-changed', { track_id: 99, prev_track_id: null });
    expect(harness.svc.currentTrackId()).toBe(99);

    harness.emit('playback:state-changed', { state: 'playing' });
    expect(harness.svc.state()).toBe('playing');

    harness.emit('playback:position-update', { position_ms: 1500, duration_ms: 200_000 });
    expect(harness.svc.positionMs()).toBe(1500);
    expect(harness.svc.durationMs()).toBe(200_000);

    // duration_ms = 0 must not overwrite a known duration.
    harness.emit('playback:position-update', { position_ms: 1700, duration_ms: 0 });
    expect(harness.svc.durationMs()).toBe(200_000);

    harness.emit('playback:volume-changed', { volume: 73 });
    expect(harness.svc.volume()).toBe(73);
  });

  it('auto-advances on track-ended', async () => {
    const harness = build();
    await harness.ready;
    harness.svc.enqueue({ ...TRACK, id: 9 });
    harness.emit('playback:track-ended', { track_id: 1 });
    // advanceFromQueue is fire-and-forget inside the listener; let
    // microtasks settle before asserting on the queue.
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 9 });
  });

  it('routes tray + MPRIS commands through the state machine', async () => {
    const harness = build();
    await harness.ready;
    harness.svc.state.set('playing');
    harness.emit('tray:toggle-play', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('pause');

    harness.invoke.mockClear();
    harness.emit('mpris:play', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('resume');

    harness.invoke.mockClear();
    harness.emit('mpris:pause', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('pause');

    harness.invoke.mockClear();
    harness.emit('mpris:stop', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('stop');

    // mpris:play-pause goes through togglePlay (state already 'paused'
    // after the pause above). Resume should fire.
    harness.svc.state.set('paused');
    harness.invoke.mockClear();
    harness.emit('mpris:play-pause', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('resume');

    // tray:next + mpris:next both pull from the queue.
    harness.svc.queue.set([{ ...TRACK, id: 100 }]);
    harness.invoke.mockClear();
    harness.emit('tray:next', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 100 });

    harness.svc.queue.set([{ ...TRACK, id: 101 }]);
    harness.invoke.mockClear();
    harness.emit('mpris:next', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 101 });
  });

  it('mpris:seek translates microseconds offset to absolute ms seek', async () => {
    const harness = build();
    await harness.ready;
    harness.svc.positionMs.set(5000);
    harness.emit('mpris:seek', 2_000_000); // +2s
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('seek', { positionMs: 7000 });
  });

  it('mpris:set-position translates microseconds to absolute ms', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('mpris:set-position', 3_000_000);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('seek', { positionMs: 3000 });
  });

  it('mpris:set-volume forwards percent to set_volume', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('mpris:set-volume', 42);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('set_volume', { volume: 42 });
  });

  it('ngOnDestroy() invokes every captured unlistener', async () => {
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
        { provide: PlaybackService, useClass: PlaybackService },
      ],
    });
    const svc = runInInjectionContext(injector, () => injector.get(PlaybackService));
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
    expect(unlistenSpies.length).toBeGreaterThan(0);
    svc.ngOnDestroy();
    for (const u of unlistenSpies) expect(u).toHaveBeenCalledTimes(1);
    svc.ngOnDestroy();
    for (const u of unlistenSpies) expect(u).toHaveBeenCalledTimes(1);
  });
});
