import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import {
  LibraryService,
  type AlbumSummary,
  type ArtistSummary,
} from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { ArtistSplitViewComponent } from './artist-split-view.component';

interface ArtistInternals {
  selected: { (): string | null; set(v: string | null): void };
  tracks: { (): TrackRow[] };
  albumsForSelected: { (): AlbumSummary[] };
  trackByArtist(i: number, a: ArtistSummary): string;
  trackByAlbum(i: number, a: AlbumSummary): string;
  trackByTrack(i: number, t: TrackRow): number;
  select(a: ArtistSummary): Promise<void>;
  play(t: TrackRow): Promise<void>;
  formatDuration(ms: number): string;
}

const ALBUM = (overrides: Partial<AlbumSummary> = {}): AlbumSummary => ({
  album: 'A',
  albumArtist: 'AA',
  year: null,
  trackCount: 1,
  totalDurationMs: 1000,
  artworkPath: null,
  ...overrides,
});

const TRACK = (id: number): TrackRow => ({
  id,
  title: `T${id}`,
  artist: null,
  album: null,
  durationMs: 1000,
  filePath: '/x',
  sampleRate: null,
  bitDepth: null,
  kind: null,
  playCount: 0,
  skipCount: 0,
});

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [ArtistSplitViewComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(ArtistSplitViewComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as ArtistInternals,
    library: TestBed.inject(LibraryService),
    playback: TestBed.inject(PlaybackService),
  };
}

describe('ArtistSplitViewComponent', () => {
  it('mounts without errors and seeds defaults', () => {
    const { cmp } = setup();
    expect(cmp.selected()).toBeNull();
    expect(cmp.tracks()).toEqual([]);
    expect(cmp.albumsForSelected()).toEqual([]);
  });

  it('trackBy helpers produce stable keys', () => {
    const { cmp } = setup();
    expect(cmp.trackByArtist(0, { artist: 'X', albumCount: 1, trackCount: 1 })).toBe('X');
    expect(cmp.trackByAlbum(0, ALBUM({ albumArtist: 'A', album: 'B' }))).toBe('A B');
    expect(cmp.trackByTrack(0, TRACK(7))).toBe(7);
  });

  it('formatDuration uses formatMmSs', () => {
    const { cmp } = setup();
    expect(cmp.formatDuration(65_000)).toBe('1:05');
  });

  it('albumsForSelected filters by selected artist', () => {
    const { cmp, library } = setup();
    library.albums.set([
      ALBUM({ albumArtist: 'X', album: 'Album1' }),
      ALBUM({ albumArtist: 'Y', album: 'Album2' }),
    ]);
    cmp.selected.set('X');
    expect(cmp.albumsForSelected()).toHaveLength(1);
    expect(cmp.albumsForSelected()[0].album).toBe('Album1');
  });

  it('select() sets selected, fetches each album’s tracks, flattens', async () => {
    const { cmp, library } = setup();
    library.albums.set([
      ALBUM({ albumArtist: 'X', album: 'A1' }),
      ALBUM({ albumArtist: 'X', album: 'A2' }),
      ALBUM({ albumArtist: 'Y', album: 'B1' }),
    ]);
    const fetchSpy = vi
      .spyOn(library, 'tracksForAlbum')
      .mockImplementation(async (_a, album) =>
        album === 'A1' ? [TRACK(1)] : album === 'A2' ? [TRACK(2)] : [TRACK(3)],
      );
    await cmp.select({ artist: 'X', albumCount: 2, trackCount: 2 });
    expect(cmp.selected()).toBe('X');
    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(cmp.tracks().map((t) => t.id)).toEqual([1, 2]);
  });

  it('play() forwards to PlaybackService', async () => {
    const { cmp, playback } = setup();
    const spy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.play(TRACK(9));
    expect(spy).toHaveBeenCalledWith(9);
  });
});
