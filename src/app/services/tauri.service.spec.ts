import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TauriService } from './tauri.service';

/**
 * These tests drive the *real* `@tauri-apps/api` by installing the
 * `__TAURI_INTERNALS__` bridge the webview normally provides.
 *
 * An earlier version mocked the two API modules with `vi.mock`, which
 * failed intermittently (roughly 1 run in 6 under CPU contention): when
 * the hoisted factory did not take effect, the real module loaded and
 * hit an undefined `window.__TAURI_INTERNALS__`. Stubbing the bridge
 * has no module-resolution dependency, so it cannot race — and it
 * covers more, since the genuine `invoke` and `listen` now run.
 */
interface Internals {
  invoke: ReturnType<typeof vi.fn>;
  transformCallback: ReturnType<typeof vi.fn>;
}

/** The event plugin keeps its own bridge for listener bookkeeping. */
interface EventInternals {
  unregisterListener: ReturnType<typeof vi.fn>;
}

interface GlobalWithTauri {
  __TAURI_INTERNALS__?: Internals;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: EventInternals;
}

/** Callbacks registered through `transformCallback`, keyed by id. */
let callbacks: Map<number, (payload: unknown) => void>;
let internals: Internals;
let eventInternals: EventInternals;

beforeEach(() => {
  callbacks = new Map();
  let nextId = 1;
  internals = {
    transformCallback: vi.fn((cb: (payload: unknown) => void) => {
      const id = nextId;
      nextId += 1;
      callbacks.set(id, cb);
      return id;
    }),
    invoke: vi.fn(async () => undefined),
  };
  eventInternals = { unregisterListener: vi.fn() };
  (globalThis as GlobalWithTauri).__TAURI_INTERNALS__ = internals;
  (globalThis as GlobalWithTauri).__TAURI_EVENT_PLUGIN_INTERNALS__ = eventInternals;
});

afterEach(() => {
  delete (globalThis as GlobalWithTauri).__TAURI_INTERNALS__;
  delete (globalThis as GlobalWithTauri).__TAURI_EVENT_PLUGIN_INTERNALS__;
});

describe('TauriService', () => {
  it('invoke() forwards command + args to @tauri-apps/api/core', async () => {
    internals.invoke.mockResolvedValueOnce(42);
    const svc = new TauriService();

    const out = await svc.invoke<number>('answer', { question: 'life' });

    expect(out).toBe(42);
    expect(internals.invoke).toHaveBeenCalledWith('answer', { question: 'life' }, undefined);
  });

  it('invoke() with no args still reaches the backend', async () => {
    internals.invoke.mockResolvedValueOnce('ok');
    const svc = new TauriService();

    await expect(svc.invoke<string>('ping')).resolves.toBe('ok');
    // The API defaults a missing argument object to `{}`.
    expect(internals.invoke).toHaveBeenCalledWith('ping', {}, undefined);
  });

  it('invoke() propagates a backend rejection', async () => {
    internals.invoke.mockRejectedValueOnce('boom');
    const svc = new TauriService();

    await expect(svc.invoke('explode')).rejects.toBe('boom');
  });

  it('listen() unwraps the event envelope before calling the handler', async () => {
    // The real `listen` registers the handler and asks the backend to
    // start the subscription; the id it resolves is the event id.
    internals.invoke.mockResolvedValueOnce(7);
    const svc = new TauriService();
    const handler = vi.fn();

    const off = await svc.listen<{ value: number }>('chan', handler);

    expect(internals.invoke).toHaveBeenCalledWith(
      'plugin:event|listen',
      expect.objectContaining({ event: 'chan' }),
      undefined,
    );

    // Deliver an event the way the Rust side would.
    const registered = callbacks.get(1);
    expect(registered).toBeDefined();
    registered?.({ event: 'chan', id: 1, payload: { value: 7 } });

    expect(handler).toHaveBeenCalledWith({ value: 7 });
    expect(typeof off).toBe('function');
  });

  it('the returned unlisten function tells the backend to stop', async () => {
    internals.invoke.mockResolvedValueOnce(7);
    const svc = new TauriService();
    const off = await svc.listen('chan', vi.fn());

    internals.invoke.mockClear();
    // `UnlistenFn` is typed `() => void` even though the API's
    // implementation is async, so let its work settle rather than
    // awaiting a value the type says is not a promise.
    off();
    for (let i = 0; i < 10; i += 1) await Promise.resolve();

    expect(eventInternals.unregisterListener).toHaveBeenCalledWith('chan', 7);
    expect(internals.invoke).toHaveBeenCalledWith(
      'plugin:event|unlisten',
      expect.objectContaining({ event: 'chan', eventId: 7 }),
      undefined,
    );
  });
});
