import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { LibraryService, type AlbumSummary } from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { AlbumGridViewComponent } from './album-grid-view.component';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (p: string) => `asset://${p}`,
  invoke: vi.fn(async () => undefined),
}));

const ALBUM = (overrides: Partial<AlbumSummary> = {}): AlbumSummary => ({
  album: 'A',
  albumArtist: 'AA',
  year: 2020,
  trackCount: 2,
  totalDurationMs: 60_000,
  artworkPath: '/cov.jpg',
  ...overrides,
});

const TRACK = (id: number): TrackRow => ({
  id,
  title: `T${id}`,
  artist: null,
  album: null,
  durationMs: 1000,
  filePath: '/tmp/x.flac',
  sampleRate: null,
  bitDepth: null,
  kind: null,
  playCount: 0,
  skipCount: 0,
  missing: false,
  artworkPath: null,
});

interface AlbumGridInternals {
  expanded: { (): { albumArtist: string; album: string } | null };
  expandedTracks: { (): TrackRow[]; set(v: TrackRow[]): void };
  trackByAlbum(i: number, a: AlbumSummary): string;
  trackByTrack(i: number, t: TrackRow): number;
  isExpanded(a: AlbumSummary): boolean;
  toggle(a: AlbumSummary): Promise<void>;
  play(t: TrackRow): Promise<void>;
  formatDuration(ms: number): string;
  coverUrl(p: string | null): string | null;
  onAlbumContextMenu(a: AlbumSummary, event: MouseEvent): Promise<void>;
  onTrackContextMenu(t: TrackRow, event: MouseEvent): void;
  onCardVisible(a: AlbumSummary): void;
}

function setup(invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [AlbumGridViewComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(AlbumGridViewComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as AlbumGridInternals,
    library: TestBed.inject(LibraryService),
    playback: TestBed.inject(PlaybackService),
    ctx: TestBed.inject(ContextMenuService),
    stub,
  };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 4; i += 1) await Promise.resolve();
}

describe('AlbumGridViewComponent', () => {
  it('refreshes albums on init', () => {
    const { library } = setup();
    expect(library.albums).toBeDefined();
  });

  it('trackBy helpers produce stable keys', () => {
    const { cmp } = setup();
    expect(cmp.trackByAlbum(0, ALBUM())).toBe('AA A');
    expect(cmp.trackByTrack(0, TRACK(7))).toBe(7);
  });

  it('formatDuration delegates to formatMmSs', () => {
    const { cmp } = setup();
    expect(cmp.formatDuration(65_000)).toBe('1:05');
  });

  it('coverUrl returns null for missing artwork and an asset URL otherwise', () => {
    const { cmp } = setup();
    expect(cmp.coverUrl(null)).toBeNull();
    expect(cmp.coverUrl('/cov.jpg')).toBe('asset:///cov.jpg');
  });

  it('onCardVisible resolves missing artwork once and patches the album in place', async () => {
    const { cmp, library, stub } = setup(async (cmd, args) => {
      if (cmd === 'resolve_album_artwork') {
        return (args as { album: string }).album === 'Hit' ? '/cache/abc.jpg' : null;
      }
      return [];
    });
    const hit = ALBUM({ album: 'Hit', artworkPath: null });
    const miss = ALBUM({ album: 'Miss', artworkPath: null });
    const has = ALBUM({ album: 'Has', artworkPath: '/x.jpg' });
    await flush(); // let the mount-time refreshAlbums() land first
    library.albums.set([hit, miss, has]);
    cmp.onCardVisible(hit);
    cmp.onCardVisible(miss);
    cmp.onCardVisible(has);
    cmp.onCardVisible(hit); // second sighting: no re-probe
    cmp.onCardVisible(miss); // misses are remembered too
    await flush();
    const calls = stub.invoke.mock.calls.filter((c) => c[0] === 'resolve_album_artwork');
    expect(calls).toHaveLength(2);
    expect(calls[0][1]).toEqual({ albumArtist: 'AA', album: 'Hit' });
    expect(library.albums().find((a) => a.album === 'Hit')?.artworkPath).toBe('/cache/abc.jpg');
    expect(library.albums().find((a) => a.album === 'Miss')?.artworkPath).toBeNull();
    expect(library.albums().find((a) => a.album === 'Has')?.artworkPath).toBe('/x.jpg');
  });

  it('artwork lookups are capped at 4 in flight and drain in order', async () => {
    const pending: ((v: unknown) => void)[] = [];
    const { cmp, stub } = setup(async (cmd) => {
      if (cmd === 'resolve_album_artwork') return new Promise((r) => pending.push(r));
      return [];
    });
    for (let i = 0; i < 6; i += 1) cmp.onCardVisible(ALBUM({ album: `A${i}`, artworkPath: null }));
    await flush();
    const started = () => stub.invoke.mock.calls.filter((c) => c[0] === 'resolve_album_artwork');
    expect(started()).toHaveLength(4);
    pending[0](null);
    await flush();
    expect(started()).toHaveLength(5);
    expect(started()[4][1]).toEqual({ albumArtist: 'AA', album: 'A4' });
    pending[1](null);
    pending[2](null);
    await flush();
    expect(started()).toHaveLength(6);
  });

  it('a failed artwork lookup frees its slot', async () => {
    const { cmp, stub } = setup(async (cmd) => {
      if (cmd === 'resolve_album_artwork') throw new Error('boom');
      return [];
    });
    for (let i = 0; i < 5; i += 1) cmp.onCardVisible(ALBUM({ album: `A${i}`, artworkPath: null }));
    await flush();
    expect(stub.invoke.mock.calls.filter((c) => c[0] === 'resolve_album_artwork')).toHaveLength(5);
  });

  it('toggle() expands an album, fetches tracks, and collapses on second click', async () => {
    const { cmp, library } = setup();
    const a = ALBUM();
    vi.spyOn(library, 'tracksForAlbum').mockResolvedValue([TRACK(1), TRACK(2)]);
    await cmp.toggle(a);
    expect(cmp.isExpanded(a)).toBe(true);
    expect(cmp.expandedTracks().length).toBe(2);
    await cmp.toggle(a);
    expect(cmp.isExpanded(a)).toBe(false);
    expect(cmp.expandedTracks()).toEqual([]);
  });

  it('play() forwards to PlaybackService', async () => {
    const { cmp, playback } = setup();
    const spy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.play(TRACK(1));
    expect(spy).toHaveBeenCalledWith(1);
  });

  it('onAlbumContextMenu loads tracks lazily then offers play/queue/next actions', async () => {
    const { cmp, ctx, library, playback } = setup();
    vi.spyOn(library, 'tracksForAlbum').mockResolvedValue([TRACK(1), TRACK(2)]);
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    const enqueueSpy = vi.spyOn(playback, 'enqueue');
    const playNextSpy = vi.spyOn(playback, 'playNext');
    const showSpy = vi.spyOn(ctx, 'show');
    await cmp.onAlbumContextMenu(ALBUM(), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items[0].label).toContain('Play album');

    await items[0].action?.();
    expect(playSpy).toHaveBeenCalledWith(1);
    expect(enqueueSpy).toHaveBeenCalledWith(expect.objectContaining({ id: 2 }));

    enqueueSpy.mockClear();
    items[1].action?.();
    expect(enqueueSpy).toHaveBeenCalledTimes(2);

    items[2].action?.();
    expect(playNextSpy).toHaveBeenCalledTimes(2);
  });

  it('onAlbumContextMenu uses already-loaded expanded tracks when present', async () => {
    const { cmp, ctx, library } = setup();
    const a = ALBUM();
    cmp.expandedTracks.set([TRACK(1)]);
    // Prime the expanded signal so isExpanded() returns true.
    await (async () => {
      vi.spyOn(library, 'tracksForAlbum').mockResolvedValue([TRACK(1)]);
      await cmp.toggle(a);
    })();
    const fetchSpy = vi.spyOn(library, 'tracksForAlbum');
    fetchSpy.mockClear();
    const showSpy = vi.spyOn(ctx, 'show');
    await cmp.onAlbumContextMenu(a, { preventDefault: vi.fn() } as unknown as MouseEvent);
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(showSpy).toHaveBeenCalled();
  });

  it('onAlbumContextMenu Play action is a no-op when album has no tracks', async () => {
    const { cmp, ctx, library, playback } = setup();
    vi.spyOn(library, 'tracksForAlbum').mockResolvedValue([]);
    const showSpy = vi.spyOn(ctx, 'show');
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.onAlbumContextMenu(ALBUM(), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    await items[0].action?.();
    expect(playSpy).not.toHaveBeenCalled();
  });

  it('onTrackContextMenu offers Play / Add / Play-next', () => {
    const { cmp, ctx, playback } = setup();
    const showSpy = vi.spyOn(ctx, 'show');
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    const enqueueSpy = vi.spyOn(playback, 'enqueue');
    const playNextSpy = vi.spyOn(playback, 'playNext');
    cmp.onTrackContextMenu(TRACK(5), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    items[0].action?.();
    items[1].action?.();
    items[2].action?.();
    expect(playSpy).toHaveBeenCalledWith(5);
    expect(enqueueSpy).toHaveBeenCalled();
    expect(playNextSpy).toHaveBeenCalled();
  });
});
