import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { LibraryService } from '../../services/library.service';
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

interface Internals {
  albums(): PlaylistAlbum[];
  isExpanded(a: PlaylistAlbum): boolean;
  toggle(a: PlaylistAlbum): void;
  onCardVisible(a: PlaylistAlbum): void;
  play(t: TrackRow): Promise<void>;
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

  it('matches album names case-insensitively and falls back to artist', () => {
    const rows = [
      TRACK(1, { album: 'Live', albumArtist: null, artist: 'Y' }),
      TRACK(2, { album: 'LIVE', albumArtist: null, artist: 'y' }),
    ];
    const groups = groupByAlbum(rows);
    expect(groups).toHaveLength(1);
    expect(groups[0].artist).toBe('Y');
  });

  it('collects untagged rows under Unknown Album / Unknown Artist', () => {
    const rows = [TRACK(1, { album: null, albumArtist: null, artist: null })];
    const [g] = groupByAlbum(rows);
    expect(g.album).toBe(UNKNOWN_ALBUM);
    expect(g.artist).toBe(UNKNOWN_ARTIST);
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
  it('renders one card per album and an empty state for no rows', () => {
    const { el } = setup([
      TRACK(1, { album: 'A' }),
      TRACK(2, { album: 'B' }),
      TRACK(3, { album: 'A' }),
    ]);
    expect(el.querySelectorAll('[data-album]')).toHaveLength(2);
    TestBed.resetTestingModule();
    expect(setup([]).el.textContent).toContain('This playlist is empty');
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

  it('lists the expanded album 1..N with track numbers', () => {
    const { cmp, fixture, el } = setup([
      TRACK(1, { trackNumber: 2, title: 'Second' }),
      TRACK(2, { trackNumber: 1, title: 'First' }),
    ]);
    cmp.toggle(cmp.albums()[0]);
    fixture.detectChanges();
    const rows = [...el.querySelectorAll('[data-tracks-for] li')].map((li) =>
      li.textContent?.replace(/\s+/g, ' ').trim(),
    );
    expect(rows[0]).toContain('1 First');
    expect(rows[1]).toContain('2 Second');
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
    cmp.onCardVisible(cmp.albums()[0]);
    expect(resolve).toHaveBeenCalledTimes(2);
  });

  it('coverUrl converts a path and passes null through', () => {
    const { cmp } = setup([]);
    expect(cmp.coverUrl('/c.jpg')).toBe('asset:///c.jpg');
    expect(cmp.coverUrl(null)).toBeNull();
  });

  it('double-clicking a track plays it', async () => {
    const { cmp, fixture, el, playback } = setup([TRACK(5)]);
    const play = vi.spyOn(playback, 'play').mockResolvedValue();
    cmp.toggle(cmp.albums()[0]);
    fixture.detectChanges();
    el.querySelector('[data-tracks-for] li')?.dispatchEvent(new MouseEvent('dblclick'));
    await fixture.whenStable();
    expect(play).toHaveBeenCalledWith(5);
  });

  it('playAlbum plays the first track and queues the rest in order', async () => {
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
    let items: ContextMenuItem[] = [];
    vi.spyOn(ctx, 'show').mockImplementation((_e, i) => {
      items = i;
    });
    const enqueue = vi.spyOn(playback, 'enqueue').mockImplementation(() => undefined);
    const playNext = vi.spyOn(playback, 'playNext').mockImplementation(() => undefined);
    cmp.onAlbumContextMenu(cmp.albums()[0], new MouseEvent('contextmenu'));
    expect(items.map((i) => i.label)).toEqual([
      'Play album (2)',
      'Add album to queue',
      'Play album next',
    ]);
    await items[1].action?.();
    expect(enqueue.mock.calls.map((c) => c[0].id)).toEqual([1, 2]);
    await items[2].action?.();
    expect(playNext.mock.calls.map((c) => c[0].id)).toEqual([2, 1]);
  });

  it('track context menu includes Get Info… which opens the editor', async () => {
    const { cmp, ctx, ui } = setup([TRACK(9)]);
    let items: ContextMenuItem[] = [];
    vi.spyOn(ctx, 'show').mockImplementation((_e, i) => {
      items = i;
    });
    cmp.onTrackContextMenu(TRACK(9), new MouseEvent('contextmenu'));
    expect(items.map((i) => i.label)).toEqual(['Play', 'Add to queue', 'Play next', 'Get Info…']);
    await items[3].action?.();
    expect(ui.trackInfo()).toEqual({ trackId: 9 });
  });
});
