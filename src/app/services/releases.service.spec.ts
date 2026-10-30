import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { ReleasesService, releaseKind } from './releases.service';
import { TauriService } from './tauri.service';

type Listener = (payload: unknown) => void;

const card = {
  id: 'rg1',
  title: 'Moon',
  artist: 'Björk',
  release_date: '2026-10-01',
  primary_type: 'Album',
  secondary_types: ['Live'],
  in_library: false,
  cover_url: 'https://coverartarchive.org/release-group/rg1/front-250',
  qobuz_url: 'https://play.qobuz.com/search?q=Bj%C3%B6rk%20Moon',
  qobuz_exact: false,
  musicbrainz_url: 'https://musicbrainz.org/release-group/rg1',
};

const feed = (over: Record<string, unknown> = {}) => ({
  checked_at: '2026-10-30T12:00:00Z',
  running: false,
  started: true,
  window_days: 90,
  releases: [card],
  last_error: null,
  qobuz_error: null,
  ...over,
});

function build(invokeImpl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
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
      { provide: ReleasesService, useClass: ReleasesService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(ReleasesService));
  const settle = async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  };
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  return { svc, invoke, settle, emit };
}

describe('ReleasesService', () => {
  it('maps the feed from snake_case', async () => {
    const { svc } = build(async () => feed());
    await svc.refresh();
    const f = svc.feed();
    expect(f.started).toBe(true);
    expect(f.checkedAt?.toISOString()).toBe('2026-10-30T12:00:00.000Z');
    expect(f.releases[0]).toEqual({
      id: 'rg1',
      title: 'Moon',
      artist: 'Björk',
      releaseDate: '2026-10-01',
      primaryType: 'Album',
      secondaryTypes: ['Live'],
      inLibrary: false,
      coverUrl: card.cover_url,
      qobuzUrl: card.qobuz_url,
      qobuzExact: false,
      musicbrainzUrl: card.musicbrainz_url,
    });
  });

  it('tracks a run from progress to finished and re-reads the feed', async () => {
    const { svc, invoke, settle, emit } = build(async () => feed({ checked_at: null }));
    await settle();
    await svc.checkNow();
    expect(invoke).toHaveBeenCalledWith('check_new_releases');
    expect(svc.running()).toBe(true);

    emit('releases:progress', { done: 25, total: 100, releases: 3 });
    expect(svc.progress()).toEqual({ done: 25, total: 100, releases: 3 });

    emit('releases:finished', { error: null });
    expect(svc.running()).toBe(false);
    expect(svc.progress()).toBeNull();
    await settle();
    expect(invoke).toHaveBeenCalledWith('get_new_releases');
  });

  it('clears the running flag when the check cannot start', async () => {
    const { svc } = build(async (cmd) => {
      if (cmd === 'check_new_releases') throw new Error('boom');
      return undefined;
    });
    await expect(svc.checkNow()).rejects.toThrow('boom');
    expect(svc.running()).toBe(false);
  });

  it('opens links and saves prefs through the backend', async () => {
    const { svc, invoke } = build(async (_cmd, args) => args?.['prefs']);
    await svc.open(card.qobuz_url);
    expect(invoke).toHaveBeenCalledWith('open_release_link', { url: card.qobuz_url });
    const prefs = { window_days: 30, qobuz_app_id: '123' };
    expect(await svc.setPrefs(prefs)).toEqual(prefs);
  });
});

describe('ReleasesService ordering and errors', () => {
  it('drops a feed reply that a later refresh has overtaken', async () => {
    const replies: ((v: unknown) => void)[] = [];
    const { svc } = build(
      (cmd) =>
        new Promise((resolve) => {
          if (cmd === 'get_new_releases') replies.push(resolve);
          else resolve(undefined);
        }),
    );
    const first = svc.refresh();
    const second = svc.refresh();
    replies[1](feed({ running: false }));
    await second;
    replies[0](feed({ running: true }));
    await first;
    expect(svc.running()).toBe(false);
  });

  it('drops a feed reply that was in flight when a check started', async () => {
    let reply: (v: unknown) => void = () => {};
    const { svc } = build(
      (cmd) =>
        new Promise((resolve) => {
          if (cmd === 'get_new_releases') reply = resolve;
          else resolve(true);
        }),
    );
    const pending = svc.refresh();
    await svc.checkNow();
    reply(feed({ running: false }));
    await pending;
    expect(svc.running()).toBe(true);
  });

  it('keeps the error a run finished with until the next check', async () => {
    const { svc, settle, emit } = build(async () => feed());
    await settle();
    emit('releases:finished', { error: 'musicbrainz unavailable' });
    expect(svc.runError()).toBe('musicbrainz unavailable');
    await svc.checkNow();
    expect(svc.runError()).toBeNull();
  });
});

describe('releaseKind', () => {
  it('joins the primary and secondary types', () => {
    expect(releaseKind({ primaryType: 'Album', secondaryTypes: ['Live'] })).toBe('Album · Live');
    expect(releaseKind({ primaryType: null, secondaryTypes: [] })).toBe('');
  });
});
