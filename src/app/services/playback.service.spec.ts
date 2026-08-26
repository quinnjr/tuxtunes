import { Injector, runInInjectionContext } from '@angular/core';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { LibraryService } from './library.service';
import { UiService } from './ui.service';
import { PlaybackService, type TrackRow } from './playback.service';
import { TauriService } from './tauri.service';

type Listener = (payload: unknown) => void;

interface Harness {
  svc: PlaybackService;
  library: LibraryService;
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
      { provide: LibraryService, useClass: LibraryService },
      { provide: UiService, useClass: UiService },
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

  return { svc, library: injector.get(LibraryService), invoke: invokeSpy, emit, ready };
}

const TRACK: TrackRow = {
  id: 42,
  title: 'T',
  artist: 'A',
  album: 'Al',
  albumArtist: null,
  durationMs: 180_000,
  filePath: '/tmp/a.flac',
  sampleRate: 44_100,
  bitDepth: 16,
  kind: 'flac',
  playCount: 0,
  skipCount: 0,
  missing: false,
  artworkPath: null,
};

describe('PlaybackService', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('initializes signals to defaults', () => {
    const { svc } = build();
    expect(svc.currentTrackId()).toBeNull();
    expect(svc.state()).toBe('stopped');
    expect(svc.positionMs()).toBe(0);
    expect(svc.durationMs()).toBe(0);
    expect(svc.volume()).toBe(100);
    expect(svc.queue()).toEqual([]);
  });

  it('play() failure surfaces lastError, flags the row missing, and clears on the timer', async () => {
    vi.useFakeTimers();
    const harness = build(async (cmd) => {
      if (cmd === 'play_track') throw new Error('File not found: /tmp/a.flac');
      return [];
    });
    await harness.ready;
    const { library } = harness;
    library.tracks.set([TRACK, { ...TRACK, id: 7 }]);
    harness.svc.enqueue(TRACK);

    await harness.svc.play(42);
    expect(harness.svc.lastError()).toBe('File not found: /tmp/a.flac');
    expect(library.tracks().map((t) => t.missing)).toEqual([true, false]);
    expect(harness.svc.queue()[0].missing).toBe(true);

    vi.advanceTimersByTime(6000);
    expect(harness.svc.lastError()).toBeNull();
  });

  it('play() success clears a previous error; non-missing errors do not flag rows', async () => {
    let fail = true;
    const harness = build(async (cmd) => {
      if (cmd === 'play_track' && fail) throw new Error('engine down');
      return [];
    });
    await harness.ready;
    const { library } = harness;
    library.tracks.set([TRACK]);
    await harness.svc.play(42);
    expect(harness.svc.lastError()).toBe('engine down');
    expect(library.tracks()[0].missing).toBe(false);
    fail = false;
    await harness.svc.play(42);
    expect(harness.svc.lastError()).toBeNull();
  });

  it('reads the persisted volume at startup (the restore event fires before we listen)', async () => {
    const harness = build(async (cmd) => {
      if (cmd === 'get_audio_prefs') {
        return { device_id: null, exclusive: false, replaygain_mode: 'off', volume: 37 };
      }
      return [];
    });
    await harness.ready;
    expect(harness.invoke).toHaveBeenCalledWith('get_audio_prefs');
    expect(harness.svc.volume()).toBe(37);
  });

  it('keeps the default volume when prefs carry none or the read fails', async () => {
    const a = build(async (cmd) => (cmd === 'get_audio_prefs' ? { volume: null } : []));
    await a.ready;
    expect(a.svc.volume()).toBe(100);
    const b = build(async (cmd) => {
      if (cmd === 'get_audio_prefs') throw new Error('nope');
      return [];
    });
    await b.ready;
    expect(b.svc.volume()).toBe(100);
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

  it('togglePlay() pauses while playing or loading, resumes while paused, no-ops otherwise', async () => {
    const { svc, invoke } = build();
    svc.state.set('loading');
    expect(svc.isActive()).toBe(true);
    await svc.togglePlay();
    expect(invoke).toHaveBeenCalledWith('pause');
    invoke.mockClear();
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

  it('previous() restarts the current track past the 3s grace window', async () => {
    const { svc, invoke } = build();
    svc.previousTrackId.set(41);
    svc.positionMs.set(3001);
    await svc.previous();
    expect(invoke).toHaveBeenCalledWith('seek', { positionMs: 0 });
    expect(invoke).not.toHaveBeenCalledWith('play_track', { trackId: 41 });
  });

  it('previous() goes to the previous track within the grace window', async () => {
    const { svc, invoke } = build();
    svc.previousTrackId.set(41);
    svc.positionMs.set(1500);
    await svc.previous();
    expect(invoke).toHaveBeenCalledWith('play_track', { trackId: 41 });
  });

  it('previous() restarts when there is no previous track, even within the grace window', async () => {
    const { svc, invoke } = build();
    svc.previousTrackId.set(null);
    svc.positionMs.set(1500);
    await svc.previous();
    expect(invoke).toHaveBeenCalledWith('seek', { positionMs: 0 });
  });

  it('listens for engine events and updates state signals', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('playback:track-changed', { track_id: 99, prev_track_id: 12 });
    expect(harness.svc.currentTrackId()).toBe(99);
    expect(harness.svc.previousTrackId()).toBe(12);

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

  it('next() falls through to the row after the current track, skipping missing rows', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
      { ...TRACK, id: 3, missing: true },
      { ...TRACK, id: 4 },
    ]);
    harness.svc.currentTrackId.set(2);
    const started = await harness.svc.next();
    expect(started?.id).toBe(4);
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 4 });

    // End of list → nothing.
    harness.svc.currentTrackId.set(4);
    harness.invoke.mockClear();
    expect(await harness.svc.next()).toBeNull();
    expect(harness.invoke).not.toHaveBeenCalledWith('play_track', expect.anything());

    // Current track not in the visible list (user switched views) → nothing.
    harness.svc.currentTrackId.set(999);
    expect(await harness.svc.next()).toBeNull();

    // Nothing playing yet → start from the top.
    harness.svc.currentTrackId.set(null);
    const fromTop = await harness.svc.next();
    expect(fromTop?.id).toBe(1);

    // Queue still wins.
    harness.svc.currentTrackId.set(1);
    harness.svc.queue.set([{ ...TRACK, id: 50 }]);
    const fromQueue = await harness.svc.next();
    expect(fromQueue?.id).toBe(50);
  });

  it('track-ended still advances when track-changed:null lands before next() resumes', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.svc.currentTrackId.set(1);
    // The engine's exact EOF sequence, delivered back-to-back.
    harness.emit('playback:track-ended', { track_id: 1 });
    harness.emit('playback:state-changed', { state: 'stopped' });
    harness.emit('playback:track-changed', { track_id: null, prev_track_id: 1 });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
  });

  it('next() skips rows whose file fails to start and keeps going', async () => {
    const harness = build(async (cmd, args) => {
      if (cmd === 'play_track' && (args as { trackId: number }).trackId === 2) {
        throw new Error('File not found: /gone.mp3');
      }
      return [];
    });
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
      { ...TRACK, id: 3 },
    ]);
    const started = await harness.svc.next(1);
    expect(started?.id).toBe(3);
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 3 });
    expect(harness.library.tracks()[1].missing).toBe(true);
    expect(harness.svc.lastError()).toBeNull(); // cleared by the successful start
  });

  it('prefetches the next list row on track-changed, and skips next() when the engine rolled into it', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2, missing: true },
      { ...TRACK, id: 3 },
    ]);
    harness.emit('playback:track-changed', { track_id: 1, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('prefetch_next', { trackId: 3 });
    harness.invoke.mockClear();
    harness.emit('playback:track-ended', { track_id: 1, next_track_id: 3 });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).not.toHaveBeenCalledWith('play_track', expect.anything());
    // At the end of the list there is nothing to pre-queue.
    harness.emit('playback:track-changed', { track_id: 3, prev_track_id: 1 });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).not.toHaveBeenCalledWith('prefetch_next', expect.anything());
    expect(harness.invoke).not.toHaveBeenCalledWith('clear_prefetch');
  });

  it('clears a stale prefetch when the next candidate disappears', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.emit('playback:track-changed', { track_id: 1, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('prefetch_next', { trackId: 2 });
    // The list changed under us: only the current track remains.
    harness.library.tracks.set([{ ...TRACK, id: 1 }]);
    harness.emit('playback:track-changed', { track_id: 1, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('clear_prefetch');
  });

  it('prefetches the queue head and pops it once the engine has switched to it', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.svc.currentTrackId.set(1);
    harness.svc.enqueue({ ...TRACK, id: 50 });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('prefetch_next', { trackId: 50 });
    harness.emit('playback:track-ended', { track_id: 1, next_track_id: 50 });
    expect(harness.svc.queue()).toEqual([]);
  });

  it('a failed prefetch falls back to the normal EOF advance', async () => {
    const harness = build(async (cmd) => {
      if (cmd === 'prefetch_next') throw new Error('File not found');
      return [];
    });
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.emit('playback:track-changed', { track_id: 1, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    harness.emit('playback:track-ended', { track_id: 1, next_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
  });

  it('track-ended continues down the list when the queue is empty', async () => {
    const harness = build();
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.svc.currentTrackId.set(1);
    harness.emit('playback:track-ended', { track_id: 1 });
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
  });

  it('track-changed resolves artwork for the new track when none is cached', async () => {
    const harness = build(async (cmd, args) => {
      if (cmd === 'resolve_track_artwork') {
        return (args as { trackId: number }).trackId === 2 ? '/cache/x.jpg' : null;
      }
      return [];
    });
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1, artworkPath: '/have.jpg' },
      { ...TRACK, id: 2 },
    ]);
    harness.emit('playback:track-changed', { track_id: 1, prev_track_id: null });
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).not.toHaveBeenCalledWith('resolve_track_artwork', expect.anything());
    expect(harness.svc.currentArtworkPath()).toBe('/have.jpg');

    harness.emit('playback:track-changed', { track_id: 2, prev_track_id: 1 });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('resolve_track_artwork', { trackId: 2 });
    expect(harness.svc.currentArtworkPath()).toBe('/cache/x.jpg');
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

    // mpris:previous routes through the same restart-vs-go-back logic.
    harness.svc.previousTrackId.set(41);
    harness.svc.positionMs.set(500);
    harness.invoke.mockClear();
    harness.emit('mpris:previous', null);
    for (let i = 0; i < 3; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 41 });
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
        { provide: LibraryService, useClass: LibraryService },
        { provide: UiService, useClass: UiService },
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

  it('pause / resume / stop / seek / setVolume resolve without throwing and surface lastError on rejection', async () => {
    const failingCommands = new Set(['pause', 'resume', 'stop', 'seek', 'set_volume']);
    const { svc } = build(async (cmd) => {
      if (failingCommands.has(cmd)) throw new Error('engine down');
      return [];
    });

    await expect(svc.pause()).resolves.toBeUndefined();
    expect(svc.lastError()).toBe('engine down');

    await expect(svc.resume()).resolves.toBeUndefined();
    expect(svc.lastError()).toBe('engine down');

    await expect(svc.stop()).resolves.toBeUndefined();
    expect(svc.lastError()).toBe('engine down');

    await expect(svc.seek(0)).resolves.toBeUndefined();
    expect(svc.lastError()).toBe('engine down');

    await expect(svc.setVolume(50)).resolves.toBeUndefined();
    expect(svc.lastError()).toBe('engine down');
  });

  it('playback:warning reports a human message through UiService', async () => {
    const harness = build();
    await harness.ready;
    harness.emit('playback:warning', {
      kind: 'sample_rate_mismatch',
      detail: 'requested 96000, got 48000',
    });
    expect(harness.svc.lastError()).toBe('Sample rate mismatch: requested 96000, got 48000');
  });

  it('playback:device-changed updates currentDevice', async () => {
    const harness = build();
    await harness.ready;
    expect(harness.svc.currentDevice()).toBeNull();
    harness.emit('playback:device-changed', {
      device_id: 'hw:0,0',
      sample_rate: 96_000,
      bit_depth: 24,
      exclusive: true,
    });
    expect(harness.svc.currentDevice()).toEqual({
      deviceId: 'hw:0,0',
      sampleRate: 96_000,
      bitDepth: 24,
      exclusive: true,
    });
  });

  it('currentArtworkPath falls back to a resolved lookup for rows not in library.tracks, and does not re-invoke on a repeat track-changed', async () => {
    const harness = build(async (cmd, args) => {
      if (cmd === 'resolve_track_artwork') {
        return (args as { trackId: number }).trackId === 7 ? '/cache/queue.jpg' : null;
      }
      return [];
    });
    await harness.ready;
    // Not loaded into library.tracks — as with queue / album-grid playback.
    expect(harness.library.tracks()).toEqual([]);

    harness.emit('playback:track-changed', { track_id: 7, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledWith('resolve_track_artwork', { trackId: 7 });
    expect(harness.svc.currentArtworkPath()).toBe('/cache/queue.jpg');

    harness.invoke.mockClear();
    harness.emit('playback:track-changed', { track_id: 7, prev_track_id: null });
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
    expect(harness.invoke).not.toHaveBeenCalledWith('resolve_track_artwork', expect.anything());
    expect(harness.svc.currentArtworkPath()).toBe('/cache/queue.jpg');
  });

  it('advanceFromQueue() skips a failing head and starts the next queued track', async () => {
    const harness = build(async (cmd, args) => {
      if (cmd === 'play_track' && (args as { trackId: number }).trackId === 1) {
        throw new Error('File not found: /gone.mp3');
      }
      return [];
    });
    await harness.ready;
    harness.svc.queue.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    const started = await harness.svc.advanceFromQueue();
    expect(started?.id).toBe(2);
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 1 });
    expect(harness.invoke).toHaveBeenCalledWith('play_track', { trackId: 2 });
    expect(harness.svc.queue()).toEqual([]);
  });

  it('next() drains a failing queue head then falls through to the list', async () => {
    const harness = build(async (cmd, args) => {
      if (cmd === 'play_track' && (args as { trackId: number }).trackId === 1) {
        throw new Error('File not found: /gone.mp3');
      }
      return [];
    });
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
    ]);
    harness.svc.currentTrackId.set(1);
    harness.svc.queue.set([{ ...TRACK, id: 1 }]);
    const started = await harness.svc.next();
    expect(started?.id).toBe(2);
    expect(harness.svc.queue()).toEqual([]);
  });

  it('next() skip-walk aborts when a newer play() supersedes it mid-walk', async () => {
    let resolveDeferred: (() => void) | undefined;
    const deferred = new Promise<void>((resolve) => {
      resolveDeferred = resolve;
    });
    const harness = build(async (cmd, args) => {
      if (cmd === 'play_track') {
        const trackId = (args as { trackId: number }).trackId;
        if (trackId === 2) {
          await deferred;
          throw new Error('File not found: /gone.mp3');
        }
      }
      return [];
    });
    await harness.ready;
    harness.library.tracks.set([
      { ...TRACK, id: 1 },
      { ...TRACK, id: 2 },
      { ...TRACK, id: 3 },
    ]);
    const nextResult = harness.svc.next(1);
    // Let the walk reach the awaited (deferred) play_track(2) call.
    for (let i = 0; i < 6; i += 1) await Promise.resolve();
    await harness.svc.play(99);
    resolveDeferred?.();
    expect(await nextResult).toBeNull();
    expect(harness.invoke).not.toHaveBeenCalledWith('play_track', { trackId: 3 });
  });

  it('next() skip-walk caps at 25 consecutive failures and reports once', async () => {
    const harness = build(async (cmd) => {
      if (cmd === 'play_track') throw new Error('File not found: /gone.mp3');
      return [];
    });
    await harness.ready;
    const rows = Array.from({ length: 30 }, (_, i) => ({ ...TRACK, id: i + 1 }));
    harness.library.tracks.set(rows);
    const started = await harness.svc.next(null);
    expect(started).toBeNull();
    expect(harness.svc.lastError()).toBe('Stopped: 25 tracks in a row could not be played');
    const playTrackCalls = harness.invoke.mock.calls.filter(([cmd]) => cmd === 'play_track');
    expect(playTrackCalls.length).toBe(25);
  });

  it('reorderQueue() is a no-op for out-of-range indices', () => {
    const { svc } = build();
    const a = { ...TRACK, id: 1 };
    const b = { ...TRACK, id: 2 };
    svc.enqueue(a);
    svc.enqueue(b);

    svc.reorderQueue(99, 0);
    expect(svc.queue().map((t) => t.id)).toEqual([1, 2]);
    expect(svc.queue().includes(undefined as unknown as TrackRow)).toBe(false);

    svc.reorderQueue(-1, 0);
    expect(svc.queue().map((t) => t.id)).toEqual([1, 2]);

    svc.reorderQueue(0, 5);
    expect(svc.queue().map((t) => t.id)).toEqual([1, 2]);
  });
});
