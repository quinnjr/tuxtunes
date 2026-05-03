import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from './library.service';
import { TauriService } from './tauri.service';

type InvokeMock = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

function build(invoke: InvokeMock): {
  svc: LibraryService;
  invoke: ReturnType<typeof vi.fn>;
} {
  const invokeSpy = vi.fn(invoke as never);
  const stubTauri = { invoke: invokeSpy } as unknown as TauriService;
  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: LibraryService, useClass: LibraryService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(LibraryService));
  return { svc, invoke: invokeSpy };
}

const RAW_TRACK = {
  id: 1,
  title: 'Title',
  artist: 'Artist',
  album: 'Album',
  duration_ms: 180_000,
  file_path: '/tmp/a.flac',
  sample_rate: 44_100,
  bit_depth: 16,
  kind: 'flac',
  play_count: 0,
  skip_count: 0,
};

describe('LibraryService', () => {
  it('initializes signals to defaults', () => {
    const { svc } = build(async () => {});
    expect(svc.stats()).toBeNull();
    expect(svc.tracks()).toEqual([]);
    expect(svc.albums()).toEqual([]);
    expect(svc.artists()).toEqual([]);
    expect(svc.search()).toBe('');
    expect(svc.filters().search).toBeNull();
    expect(svc.sort().column).toBe('date_added');
    expect(svc.sort().descending).toBe(true);
  });

  it('refreshStats() maps snake_case payload to the camelCase signal', async () => {
    const { svc } = build(async () => ({
      track_count: 7,
      total_duration_ms: 1_000_000,
      total_size_bytes: 1024,
    }));
    await svc.refreshStats();
    expect(svc.stats()).toEqual({
      trackCount: 7,
      totalDurationMs: 1_000_000,
      totalSizeBytes: 1024,
    });
  });

  it('refreshTracks() forwards filters + sort and maps rows', async () => {
    const { svc, invoke } = build(async () => [RAW_TRACK]);
    await svc.refreshTracks();
    expect(invoke).toHaveBeenCalledWith('list_tracks', {
      limit: 500,
      offset: 0,
      filters: svc.filters(),
      sort: svc.sort(),
    });
    expect(svc.tracks()).toHaveLength(1);
    expect(svc.tracks()[0].title).toBe('Title');
    expect(svc.tracks()[0].durationMs).toBe(180_000);
  });

  it('setSearch() trims input and writes to filters.search', () => {
    const { svc } = build(async () => {});
    svc.setSearch('  hello  ');
    expect(svc.search()).toBe('  hello  ');
    expect(svc.filters().search).toBe('hello');
    svc.setSearch('   ');
    expect(svc.filters().search).toBeNull();
  });

  it('cycleSort() flips direction on re-click and resets ASC on switch', async () => {
    const { svc } = build(async () => []);
    await svc.cycleSort('title'); // Switch column → ASC.
    expect(svc.sort()).toEqual({ column: 'title', descending: false });
    await svc.cycleSort('title'); // Same column → flip.
    expect(svc.sort()).toEqual({ column: 'title', descending: true });
    await svc.cycleSort('artist'); // Different column → ASC.
    expect(svc.sort()).toEqual({ column: 'artist', descending: false });
  });

  it('getDistinct() forwards the column + filters', async () => {
    const { svc, invoke } = build(async () => [{ value: 'Rock', count: 5 }]);
    const out = await svc.getDistinct('genre');
    expect(invoke).toHaveBeenCalledWith('get_distinct', {
      column: 'genre',
      filters: svc.filters(),
    });
    expect(out).toEqual([{ value: 'Rock', count: 5 }]);
  });

  it('addTrackFromPicker() returns null when the user cancels', async () => {
    const { svc } = build(async () => null);
    const out = await svc.addTrackFromPicker();
    expect(out).toBeNull();
    expect(svc.tracks()).toHaveLength(0);
  });

  it('addTrackFromPicker() prepends new tracks and refreshes stats', async () => {
    const responses: Record<string, unknown> = {
      pick_and_add_track: RAW_TRACK,
      get_library_stats: { track_count: 1, total_duration_ms: 0, total_size_bytes: 0 },
    };
    const { svc } = build(async (cmd) => responses[cmd]);
    const out = await svc.addTrackFromPicker();
    expect(out).not.toBeNull();
    expect(svc.tracks()).toHaveLength(1);
    expect(svc.stats()?.trackCount).toBe(1);
  });

  it('refreshAlbums() camelCases album rows', async () => {
    const { svc } = build(async () => [
      {
        album: 'A',
        album_artist: 'AA',
        year: 2020,
        track_count: 5,
        total_duration_ms: 60_000,
        artwork_path: '/cov.jpg',
      },
    ]);
    await svc.refreshAlbums();
    expect(svc.albums()[0]).toEqual({
      album: 'A',
      albumArtist: 'AA',
      year: 2020,
      trackCount: 5,
      totalDurationMs: 60_000,
      artworkPath: '/cov.jpg',
    });
  });

  it('refreshArtists() camelCases artist rows', async () => {
    const { svc } = build(async () => [{ artist: 'X', album_count: 2, track_count: 12 }]);
    await svc.refreshArtists();
    expect(svc.artists()[0]).toEqual({ artist: 'X', albumCount: 2, trackCount: 12 });
  });

  it('tracksForAlbum() maps the camelCase rows', async () => {
    const { svc, invoke } = build(async () => [RAW_TRACK]);
    const rows = await svc.tracksForAlbum('AA', 'A');
    expect(invoke).toHaveBeenCalledWith('tracks_for_album', {
      albumArtist: 'AA',
      album: 'A',
    });
    expect(rows[0].title).toBe('Title');
  });

  it('tracksById() rebuilds on every tracks() mutation', () => {
    const { svc } = build(async () => {});
    expect(svc.tracksById().size).toBe(0);
    svc.tracks.set([{ ...RAW_TRACK, id: 1 } as never, { ...RAW_TRACK, id: 2 } as never]);
    expect(svc.tracksById().size).toBe(2);
    expect(svc.tracksById().get(1)?.id).toBe(1);
  });
});
