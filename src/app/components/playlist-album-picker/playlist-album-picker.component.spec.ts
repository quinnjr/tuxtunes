import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { LibraryService, type Playlist } from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import {
  PlaylistAlbumPickerComponent,
  UNKNOWN_ALBUM,
  UNKNOWN_ARTIST,
  groupByAlbum,
  type PlaylistAlbum,
} from './playlist-album-picker.component';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (p: string) => `asset://${p}`,
  invoke: vi.fn(async () => undefined),
}));

const TRACK = (id: number, overrides: Partial<TrackRow> = {}): TrackRow => ({
  id,
  title: `T${id}`,
  artist: 'Artist',
  album: 'Album',
  albumArtist: 'Artist',
  genre: null,
  year: 2001,
  trackNumber: null,
  discNumber: null,
  durationMs: 1000,
  filePath: `/tmp/${id}.flac`,
  sampleRate: null,
  bitDepth: null,
  kind: null,
  playCount: 0,
  skipCount: 0,
  missing: false,
  artworkPath: null,
  ...overrides,
});

const PLAYLIST = (overrides: Partial<Playlist> = {}): Playlist => ({
  id: 3,
  name: 'Mix',
  kind: 'regular',
  parentId: null,
  sortOrder: 0,
  trackCount: 1,
  synced: false,
  ...overrides,
});

interface Internals {
  albums(): PlaylistAlbum[];
  isExpanded(a: PlaylistAlbum): boolean;
  toggle(a: PlaylistAlbum): void;
  onCardVisible(a: PlaylistAlbum): void;
  play(t: TrackRow): Promise<void>;
  playFrom(a: PlaylistAlbum, t: TrackRow): Promise<void>;
  playAlbum(a: PlaylistAlbum): Promise<void>;
  onAlbumContextMenu(a: PlaylistAlbum, event: MouseEvent): void;
  onTrackContextMenu(t: TrackRow, event: MouseEvent): void;
  coverUrl(p: string | null): string | null;
}

/**
 * `beforeRender` runs after the services exist but before the first
 * change detection: the in-view directive fires synchronously without
 * an IntersectionObserver, so artwork spies must be in place by then.
 */
function setup(rows: TrackRow[], beforeRender: (library: LibraryService) => void = () => {}) {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [PlaylistAlbumPickerComponent],
    providers: appProviders(stub),
  });
  const library = TestBed.inject(LibraryService);
  library.tracks.set(rows);
  beforeRender(library);
  const fixture = TestBed.createComponent(PlaylistAlbumPickerComponent);
  fixture.detectChanges();
  return {
    fixture,
    stub,
    library,
    cmp: fixture.componentInstance as unknown as Internals,
    el: fixture.nativeElement as HTMLElement,
    playback: TestBed.inject(PlaybackService),
    ctx: TestBed.inject(ContextMenuService),
    ui: TestBed.inject(UiService),
  };
}

function captureMenu(ctx: ContextMenuService): () => ContextMenuItem[] {
  let items: ContextMenuItem[] = [];
  vi.spyOn(ctx, 'show').mockImplementation((_e, i) => {
    items = i;
  });
  return () => items;
}

describe('groupByAlbum', () => {
  it('groups by album artist + album in order of first appearance', () => {
    const rows = [
      TRACK(1, { album: 'B', albumArtist: 'X' }),
      TRACK(2, { album: 'A', albumArtist: 'X' }),
      TRACK(3, { album: 'B', albumArtist: 'X' }),
    ];
    const groups = groupByAlbum(rows);
    expect(groups.map((g) => g.album)).toEqual(['B', 'A']);
    expect(groups[0].tracks.map((t) => t.id)).toEqual([1, 3]);
    expect(groups[0].totalDurationMs).toBe(2000);
  });

  it('orders tracks 1..N by disc then track number regardless of playlist order', () => {
    const rows = [
      TRACK(1, { discNumber: 1, trackNumber: 3 }),
      TRACK(2, { discNumber: 2, trackNumber: 1 }),
      TRACK(3, { discNumber: 1, trackNumber: 1 }),
      TRACK(4, { discNumber: 1, trackNumber: 2 }),
    ];
    expect(groupByAlbum(rows)[0].tracks.map((t) => t.id)).toEqual([3, 4, 1, 2]);
  });

  it('keeps playlist order for untagged tracks and puts them after numbered ones', () => {
    const rows = [TRACK(1), TRACK(2, { trackNumber: 5 }), TRACK(3)];
    expect(groupByAlbum(rows)[0].tracks.map((t) => t.id)).toEqual([2, 1, 3]);
  });

  it('is case-sensitive like the backend and artwork patching, falling back to artist', () => {
    const rows = [
      TRACK(1, { album: 'Live', albumArtist: null, artist: 'Y' }),
      TRACK(2, { album: 'LIVE', albumArtist: null, artist: 'Y' }),
      TRACK(3, { album: 'Live', albumArtist: null, artist: 'Y' }),
    ];
    const groups = groupByAlbum(rows);
    expect(groups.map((g) => g.tracks.map((t) => t.id))).toEqual([[1, 3], [2]]);
    expect(groups[0].artist).toBe('Y');
  });

  it('treats blank tags as missing', () => {
    const rows = [
      TRACK(1, { albumArtist: '', artist: 'Y' }),
      TRACK(2, { albumArtist: '  ', artist: 'Y' }),
      TRACK(3, { album: '', albumArtist: null, artist: null }),
    ];
    const groups = groupByAlbum(rows);
    expect(groups).toHaveLength(2);
    expect(groups[0].artist).toBe('Y');
    expect(groups[0].tracks).toHaveLength(2);
    expect(groups[1].album).toBe(UNKNOWN_ALBUM);
    expect(groups[1].artist).toBe(UNKNOWN_ARTIST);
  });

  it('keeps a duplicated track twice, as the playlist lists it', () => {
    const rows = [
      TRACK(1, { trackNumber: 1 }),
      TRACK(2, { trackNumber: 2 }),
      TRACK(1, { trackNumber: 1 }),
    ];
    const [g] = groupByAlbum(rows);
    expect(g.tracks.map((t) => t.id)).toEqual([1, 1, 2]);
  });

  it('takes artwork and year from whichever track has them', () => {
    const rows = [TRACK(1, { year: null }), TRACK(2, { artworkPath: '/c.jpg', year: 1999 })];
    const [g] = groupByAlbum(rows);
    expect(g.artworkPath).toBe('/c.jpg');
    expect(g.year).toBe(1999);
    expect(g.sampleTrackId).toBe(1);
  });
});

describe('PlaylistAlbumPickerComponent', () => {
  it('renders one card per album', () => {
    const { el } = setup([
      TRACK(1, { album: 'A' }),
      TRACK(2, { album: 'B' }),
      TRACK(3, { album: 'A' }),
    ]);
    expect(el.querySelectorAll('[data-album]')).toHaveLength(2);
  });

  it('shows an empty state, and a search-specific one while a search is active', () => {
    const { el, library, fixture } = setup([]);
    expect(el.textContent).toContain('This playlist is empty');
    library.search.set('zzz');
    fixture.detectChanges();
    expect(el.textContent).toContain('No songs match “zzz”');
  });

  it('toggle() opens and closes cards independently', () => {
    const { cmp, fixture, el } = setup([TRACK(1, { album: 'A' }), TRACK(2, { album: 'B' })]);
    const [a, b] = cmp.albums();
    cmp.toggle(a);
    cmp.toggle(b);
    fixture.detectChanges();
    expect(el.querySelectorAll('[data-tracks-for]')).toHaveLength(2);
    cmp.toggle(a);
    fixture.detectChanges();
    expect(cmp.isExpanded(a)).toBe(false);
    expect(cmp.isExpanded(b)).toBe(true);
    expect(el.querySelectorAll('[data-tracks-for]')).toHaveLength(1);
  });

  it('lists the expanded album 1..N, showing only real track numbers', () => {
    const { cmp, fixture, el } = setup([
      TRACK(1, { trackNumber: 2, title: 'Second' }),
      TRACK(2, { trackNumber: 1, title: 'First' }),
      TRACK(3, { title: 'Untagged' }),
    ]);
    cmp.toggle(cmp.albums()[0]);
    fixture.detectChanges();
    const rows = [...el.querySelectorAll('[data-tracks-for] li')].map((li) =>
      li.textContent?.replace(/\s+/g, ' ').trim(),
    );
    expect(rows[0]).toContain('1 First');
    expect(rows[1]).toContain('2 Second');
    expect(rows[2]?.startsWith('Untagged')).toBe(true);
  });

  it('renders a duplicated track twice without a keying error', () => {
    const { cmp, fixture, el } = setup([TRACK(1), TRACK(1)]);
    cmp.toggle(cmp.albums()[0]);
    fixture.detectChanges();
    expect(el.querySelectorAll('[data-tracks-for] li')).toHaveLength(2);
  });

  it('formats album totals over an hour with an hour field', () => {
    const { el } = setup([TRACK(1, { durationMs: 3_600_000 }), TRACK(2, { durationMs: 65_000 })]);
    expect(el.textContent).toContain('1:01:05');
  });

  it('recomputes when the playlist rows change', () => {
    const { cmp, library } = setup([TRACK(1, { album: 'A' })]);
    expect(cmp.albums()).toHaveLength(1);
    library.tracks.set([TRACK(1, { album: 'A' }), TRACK(2, { album: 'B' })]);
    expect(cmp.albums()).toHaveLength(2);
  });

  it('a visible card resolves artwork once via a track of the album', () => {
    let resolve!: ReturnType<typeof vi.spyOn>;
    const { cmp } = setup([TRACK(7), TRACK(8)], (library) => {
      resolve = vi.spyOn(library, 'resolveTrackArtwork').mockResolvedValue('/c.jpg');
    });
    // The directive already fired on render; a repeat is a no-op.
    cmp.onCardVisible(cmp.albums()[0]);
    expect(resolve).toHaveBeenCalledTimes(1);
    expect(resolve).toHaveBeenCalledWith(7);
  });

  it('skips artwork lookup for albums that already have it', () => {
    let resolve!: ReturnType<typeof vi.spyOn>;
    setup([TRACK(1, { artworkPath: '/c.jpg' })], (library) => {
      resolve = vi.spyOn(library, 'resolveTrackArtwork').mockResolvedValue('/c.jpg');
    });
    expect(resolve).not.toHaveBeenCalled();
  });

  it('caps concurrent artwork lookups at four and drains as they settle', async () => {
    const pending: (() => void)[] = [];
    let resolve!: ReturnType<typeof vi.spyOn>;
    setup(
      Array.from({ length: 6 }, (_, i) => TRACK(i + 1, { album: `A${i}` })),
      (library) => {
        resolve = vi.spyOn(library, 'resolveTrackArtwork').mockImplementation(
          () =>
            new Promise<string | null>((r) => {
              pending.push(() => r(null));
            }),
        );
      },
    );
    expect(resolve).toHaveBeenCalledTimes(4);
    pending[0]();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(resolve).toHaveBeenCalledTimes(5);
  });

  it('a failed artwork lookup is retried on the next visibility', async () => {
    let resolve!: ReturnType<typeof vi.spyOn>;
    const { cmp } = setup([TRACK(1)], (library) => {
      resolve = vi
        .spyOn(library, 'resolveTrackArtwork')
        .mockRejectedValueOnce(new Error('io'))
        .mockResolvedValue(null);
    });
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    cmp.onCardVisible(cmp.albums()[0]);
    expect(resolve).toHaveBeenCalledTimes(2);
  });

  it('coverUrl converts a path and passes null through', () => {
    const { cmp } = setup([]);
    expect(cmp.coverUrl('/c.jpg')).toBe('asset:///c.jpg');
    expect(cmp.coverUrl(null)).toBeNull();
  });

  it('double-clicking a track plays it and queues the rest of the card after it', async () => {
    const { cmp, fixture, el, playback } = setup([
      TRACK(1, { trackNumber: 1 }),
      TRACK(2, { trackNumber: 2 }),
      TRACK(3, { trackNumber: 3 }),
    ]);
    const play = vi.spyOn(playback, 'play').mockResolvedValue();
    const enqueue = vi.spyOn(playback, 'enqueue').mockImplementation(() => undefined);
    cmp.toggle(cmp.albums()[0]);
    fixture.detectChanges();
    el.querySelectorAll('[data-tracks-for] li')[1].dispatchEvent(new MouseEvent('dblclick'));
    await fixture.whenStable();
    expect(play).toHaveBeenCalledWith(2);
    expect(enqueue.mock.calls.map((c) => c[0].id)).toEqual([3]);
  });

  it('playAlbum plays the first track and queues the rest in card order', async () => {
    const { cmp, playback } = setup([
      TRACK(1, { trackNumber: 2 }),
      TRACK(2, { trackNumber: 1 }),
      TRACK(3, { trackNumber: 3 }),
    ]);
    const play = vi.spyOn(playback, 'play').mockResolvedValue();
    const enqueue = vi.spyOn(playback, 'enqueue').mockImplementation(() => undefined);
    await cmp.playAlbum(cmp.albums()[0]);
    expect(play).toHaveBeenCalledWith(2);
    expect(enqueue.mock.calls.map((c) => c[0].id)).toEqual([1, 3]);
  });

  it('album context menu offers play / queue / play next over the album slice', async () => {
    const { cmp, ctx, playback } = setup([
      TRACK(1, { trackNumber: 1 }),
      TRACK(2, { trackNumber: 2 }),
    ]);
    const items = captureMenu(ctx);
    const enqueue = vi.spyOn(playback, 'enqueue').mockImplementation(() => undefined);
    const playNext = vi.spyOn(playback, 'playNext').mockImplementation(() => undefined);
    cmp.onAlbumContextMenu(cmp.albums()[0], new MouseEvent('contextmenu'));
    expect(items().map((i) => i.label)).toEqual([
      'Play album (2)',
      'Add album to queue',
      'Play album next',
    ]);
    await items()[1].action?.();
    expect(enqueue.mock.calls.map((c) => c[0].id)).toEqual([1, 2]);
    await items()[2].action?.();
    expect(playNext.mock.calls.map((c) => c[0].id)).toEqual([2, 1]);
  });

  it('track context menu has Get Info… and Show in Files, no playlist removal for the library', async () => {
    const { cmp, ctx, ui, stub } = setup([TRACK(9)]);
    const items = captureMenu(ctx);
    cmp.onTrackContextMenu(TRACK(9), new MouseEvent('contextmenu'));
    const labels = items().map((i) => i.label);
    expect(labels).toEqual([
      'Play',
      'Add to queue',
      'Play next',
      '---',
      'Get Info…',
      'Show in Files',
    ]);
    await items()[4].action?.();
    expect(ui.trackInfo()).toEqual({ trackId: 9 });
    await items()[5].action?.();
    expect(stub.invoke).toHaveBeenCalledWith('show_in_files', { trackId: 9 });
  });

  it('offers Remove from Playlist only for an open, unsynced regular playlist', async () => {
    const { cmp, ctx, library } = setup([TRACK(9)]);
    const items = captureMenu(ctx);
    const remove = vi.spyOn(library, 'removeTracksFromPlaylist').mockResolvedValue();

    library.playlists.set([PLAYLIST({ synced: true })]);
    library.activePlaylistId.set(3);
    cmp.onTrackContextMenu(TRACK(9), new MouseEvent('contextmenu'));
    expect(items().map((i) => i.label)).not.toContain('Remove from Playlist');

    library.playlists.set([PLAYLIST({ kind: 'smart' })]);
    cmp.onTrackContextMenu(TRACK(9), new MouseEvent('contextmenu'));
    expect(items().map((i) => i.label)).not.toContain('Remove from Playlist');

    library.playlists.set([PLAYLIST()]);
    cmp.onTrackContextMenu(TRACK(9), new MouseEvent('contextmenu'));
    const item = items().find((i) => i.label === 'Remove from Playlist');
    expect(item).toBeDefined();
    await item?.action?.();
    expect(remove).toHaveBeenCalledWith(3, [9]);
  });
});
