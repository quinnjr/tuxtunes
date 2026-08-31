import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService, sortTracks } from './library.service';
import { mapTrack } from './playback.service';
import { TauriService } from './tauri.service';

type InvokeMock = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

function build(invoke: InvokeMock): {
  svc: LibraryService;
  invoke: ReturnType<typeof vi.fn>;
  emit: (event: string, payload?: unknown) => void;
} {
  const invokeSpy = vi.fn(invoke as never);
  const listeners = new Map<string, ((p: unknown) => void)[]>();
  const listen = vi.fn(async (event: string, h: (p: unknown) => void) => {
    listeners.set(event, [...(listeners.get(event) ?? []), h]);
    return () => undefined;
  });
  const emit = (event: string, payload: unknown = undefined) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  const stubTauri = { invoke: invokeSpy, listen } as unknown as TauriService;
  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: LibraryService, useClass: LibraryService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(LibraryService));
  return { svc, invoke: invokeSpy, emit };
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

describe('LibraryService playlists', () => {
  const raws = [
    { ...RAW_TRACK, id: 3, title: 'Zed', artist: null, album: null },
    { ...RAW_TRACK, id: 1, title: 'alpha', artist: 'B', album: null },
    { ...RAW_TRACK, id: 2, title: 'Beta', artist: 'a', album: null },
  ];

  it('refreshPlaylists maps rows and refreshTracks routes to open_playlist when active', async () => {
    const { svc, invoke } = build(async (cmd) => {
      if (cmd === 'list_playlists') {
        return [
          {
            id: 9,
            name: 'Nine',
            kind: 'regular',
            parent_id: 4,
            sort_order: 2,
            cached_track_count: 0,
            sync_source_id: 1,
          },
          {
            id: 4,
            name: 'F',
            kind: 'weird',
            parent_id: null,
            sort_order: 1,
            cached_track_count: null,
            sync_source_id: null,
          },
        ];
      }
      if (cmd === 'open_playlist') return raws;
      return [];
    });
    await svc.refreshPlaylists();
    expect(svc.playlists()).toEqual([
      {
        id: 9,
        name: 'Nine',
        kind: 'regular',
        parentId: 4,
        sortOrder: 2,
        trackCount: 0,
        synced: true,
      },
      {
        id: 4,
        name: 'F',
        kind: 'regular',
        parentId: null,
        sortOrder: 1,
        trackCount: null,
        synced: false,
      },
    ]);

    await svc.openPlaylist(9);
    expect(invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 9 });
    expect(invoke).not.toHaveBeenCalledWith('list_tracks', expect.anything());
    // Playlist order preserved, cached count refreshed from the result.
    expect(svc.tracks().map((t) => t.id)).toEqual([3, 1, 2]);
    expect(svc.activePlaylist()?.trackCount).toBe(3);

    await svc.openPlaylist(null);
    expect(invoke).toHaveBeenCalledWith('list_tracks', expect.anything());
    expect(svc.activePlaylist()).toBeNull();
  });

  it('applies search and sort client-side for an active playlist', async () => {
    const { svc } = build(async (cmd) => (cmd === 'open_playlist' ? raws : []));
    await svc.openPlaylist(9);
    svc.setSearch('a');
    await svc.refreshTracks();
    expect(svc.tracks().map((t) => t.title)).toEqual(['alpha', 'Beta']);
    svc.setSearch('');
    await svc.cycleSort('title');
    expect(svc.tracks().map((t) => t.title)).toEqual(['alpha', 'Beta', 'Zed']);
    await svc.cycleSort('title');
    expect(svc.tracks().map((t) => t.title)).toEqual(['Zed', 'Beta', 'alpha']);
  });

  it('createPlaylist invokes the command and refreshes the sidebar', async () => {
    const { svc, invoke } = build(async (cmd) => (cmd === 'create_playlist' ? 7 : []));
    const id = await svc.createPlaylist('  Mix  ');
    expect(id).toBe(7);
    expect(invoke).toHaveBeenCalledWith('create_playlist', { name: 'Mix', parentId: null });
    expect(invoke).toHaveBeenCalledWith('list_playlists');
  });

  it('createPlaylist forwards a parent folder id', async () => {
    const { svc, invoke } = build(async (cmd) => (cmd === 'create_playlist' ? 7 : []));
    await svc.createPlaylist('Inside', 4);
    expect(invoke).toHaveBeenCalledWith('create_playlist', { name: 'Inside', parentId: 4 });
  });

  it('createPlaylist still returns the id when the sidebar refresh fails', async () => {
    const { svc } = build(async (cmd) => {
      if (cmd === 'create_playlist') return 7;
      throw new Error('list_playlists exploded');
    });
    await expect(svc.createPlaylist('Mix')).resolves.toBe(7);
  });

  it('createPlaylistWithTracks creates, adds, and refreshes the sidebar once', async () => {
    const { svc, invoke } = build(async (cmd) => (cmd === 'create_playlist' ? 7 : []));
    const id = await svc.createPlaylistWithTracks('Road Trip', [3, 4]);
    expect(id).toBe(7);
    expect(invoke).toHaveBeenCalledWith('create_playlist', { name: 'Road Trip', parentId: null });
    expect(invoke).toHaveBeenCalledWith('add_tracks_to_playlist', {
      playlistId: 7,
      trackIds: [3, 4],
    });
    const refreshes = invoke.mock.calls.filter((c) => c[0] === 'list_playlists');
    expect(refreshes).toHaveLength(1);
  });

  it('addTracksToPlaylist reloads the open playlist when it is the target', async () => {
    const { svc, invoke } = build(async (cmd) => (cmd === 'open_playlist' ? raws : []));
    await svc.openPlaylist(9);
    invoke.mockClear();
    await svc.addTracksToPlaylist(9, [5]);
    expect(invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 9 });
  });

  it('renamePlaylist invokes the command and refreshes the sidebar', async () => {
    const { svc, invoke } = build(async () => []);
    await svc.renamePlaylist(9, 'Renamed');
    expect(invoke).toHaveBeenCalledWith('rename_playlist', { playlistId: 9, name: 'Renamed' });
    expect(invoke).toHaveBeenCalledWith('list_playlists');
  });

  it('addTracksToPlaylist invokes the command and refreshes the sidebar', async () => {
    const { svc, invoke } = build(async () => []);
    await svc.addTracksToPlaylist(9, [1, 2]);
    expect(invoke).toHaveBeenCalledWith('add_tracks_to_playlist', {
      playlistId: 9,
      trackIds: [1, 2],
    });
    expect(invoke).toHaveBeenCalledWith('list_playlists');
  });

  it('removeTracksFromPlaylist reloads the open playlist when it is the active one', async () => {
    const { svc, invoke } = build(async (cmd) => (cmd === 'open_playlist' ? raws : []));
    await svc.openPlaylist(9);
    invoke.mockClear();
    await svc.removeTracksFromPlaylist(9, [1]);
    expect(invoke).toHaveBeenCalledWith('remove_tracks_from_playlist', {
      playlistId: 9,
      trackIds: [1],
    });
    expect(invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 9 });
  });

  it('removeTracksFromPlaylist leaves the track list alone for an inactive playlist', async () => {
    const { svc, invoke } = build(async () => []);
    await svc.removeTracksFromPlaylist(9, [1]);
    expect(invoke).not.toHaveBeenCalledWith('open_playlist', expect.anything());
  });

  it('an external-change event refreshes playlists, tracks, and stats', async () => {
    const { svc, invoke, emit } = build(async () => []);
    await Promise.resolve(); // listener registration settles
    invoke.mockClear();
    emit('library:external-change');
    await new Promise((r) => setTimeout(r));
    expect(invoke).toHaveBeenCalledWith('list_playlists');
    expect(invoke).toHaveBeenCalledWith('list_tracks', expect.anything());
    expect(invoke).toHaveBeenCalledWith('get_library_stats');
    void svc;
  });

  it('an external-change event reloads the open playlist instead of the library query', async () => {
    const { svc, invoke, emit } = build(async (cmd) => (cmd === 'open_playlist' ? raws : []));
    await svc.openPlaylist(9);
    await Promise.resolve();
    invoke.mockClear();
    emit('library:external-change');
    await new Promise((r) => setTimeout(r));
    expect(invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 9 });
    expect(invoke).not.toHaveBeenCalledWith('list_tracks', expect.anything());
  });

  it('an external-change refresh failure is swallowed', async () => {
    const { emit } = build(async () => {
      throw new Error('db locked');
    });
    await Promise.resolve();
    expect(() => emit('library:external-change')).not.toThrow();
    await new Promise((r) => setTimeout(r));
  });

  it('ignores a stale open_playlist response after switching playlists', async () => {
    let resolveFirst: (v: unknown) => void = () => {};
    const { svc } = build(async (cmd, args) => {
      if (cmd !== 'open_playlist') return [];
      if ((args as { playlistId: number }).playlistId === 1) {
        return new Promise((r) => (resolveFirst = r));
      }
      return [raws[0]];
    });
    const first = svc.openPlaylist(1);
    await svc.openPlaylist(2);
    resolveFirst(raws);
    await first;
    expect(svc.activePlaylistId()).toBe(2);
    expect(svc.tracks().map((t) => t.id)).toEqual([3]);
  });
});

describe('sortTracks', () => {
  const rows = [
    { ...RAW_TRACK, id: 1, artist: null, play_count: 5 },
    { ...RAW_TRACK, id: 2, artist: 'b', play_count: 1 },
    { ...RAW_TRACK, id: 3, artist: 'A', play_count: 1 },
  ].map((r) => mapTrack(r));

  it('sorts strings case-insensitively with nulls last ascending', () => {
    const out = sortTracks(rows, { column: 'artist', descending: false });
    expect(out.map((t) => t.id)).toEqual([3, 2, 1]);
  });

  it('sorts numbers with nulls first descending and is stable', () => {
    const out = sortTracks(rows, { column: 'play_count', descending: true });
    expect(out.map((t) => t.id)).toEqual([1, 2, 3]);
    const asc = sortTracks(rows, { column: 'play_count', descending: false });
    expect(asc.map((t) => t.id)).toEqual([2, 3, 1]);
  });

  it('leaves order untouched for columns TrackRow does not carry', () => {
    const out = sortTracks(rows, { column: 'year', descending: false });
    expect(out.map((t) => t.id)).toEqual([1, 2, 3]);
  });
});

describe('LibraryService.resolveTrackArtwork', () => {
  it('patches the path onto the track and its album mates, returns null on a miss', async () => {
    const { svc, invoke } = build(async (cmd, args) =>
      cmd === 'resolve_track_artwork' && (args as { trackId: number }).trackId === 1
        ? '/cache/a.jpg'
        : null,
    );
    svc.tracks.set(
      [
        { ...RAW_TRACK, id: 1, album: 'Same', artist: 'X', album_artist: 'X' },
        { ...RAW_TRACK, id: 2, album: 'Same', artist: 'X', album_artist: 'X' },
        { ...RAW_TRACK, id: 3, album: 'Other', artist: 'X', album_artist: 'X' },
      ].map((r) => mapTrack(r)),
    );
    svc.albums.set([
      {
        album: 'Same',
        albumArtist: 'X',
        year: null,
        trackCount: 2,
        totalDurationMs: 0,
        artworkPath: null,
      },
      {
        album: 'Other',
        albumArtist: 'X',
        year: null,
        trackCount: 1,
        totalDurationMs: 0,
        artworkPath: null,
      },
    ]);
    expect(await svc.resolveTrackArtwork(1)).toBe('/cache/a.jpg');
    expect(invoke).toHaveBeenCalledWith('resolve_track_artwork', { trackId: 1 });
    expect(svc.tracks().map((t) => t.artworkPath)).toEqual(['/cache/a.jpg', '/cache/a.jpg', null]);
    expect(svc.albums().map((a) => a.artworkPath)).toEqual(['/cache/a.jpg', null]);
    expect(await svc.resolveTrackArtwork(3)).toBeNull();
    expect(svc.tracks()[2].artworkPath).toBeNull();
  });

  it('does not fan out onto other untagged rows', async () => {
    const { svc } = build(async (cmd, args) =>
      cmd === 'resolve_track_artwork' && (args as { trackId: number }).trackId === 1
        ? '/cache/a.jpg'
        : null,
    );
    svc.tracks.set(
      [
        { ...RAW_TRACK, id: 1, album: null, artist: null, album_artist: null },
        { ...RAW_TRACK, id: 2, album: null, artist: null, album_artist: null },
      ].map((r) => mapTrack(r)),
    );
    expect(await svc.resolveTrackArtwork(1)).toBe('/cache/a.jpg');
    expect(svc.tracks().map((t) => t.artworkPath)).toEqual(['/cache/a.jpg', null]);
  });

  it('addFolderFromPicker returns null on cancel and refreshes on success', async () => {
    let cancelled = true;
    const { svc, invoke } = build(async (cmd) => {
      if (cmd === 'pick_and_add_folder')
        return cancelled ? null : { added: 2, skipped: 1, failed: [] };
      if (cmd === 'get_library_stats')
        return { track_count: 3, total_duration_ms: 0, total_size_bytes: 0 };
      return [];
    });
    expect(await svc.addFolderFromPicker()).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith('list_tracks', expect.anything());
    cancelled = false;
    expect(await svc.addFolderFromPicker()).toEqual({ added: 2, skipped: 1, failed: [] });
    expect(invoke).toHaveBeenCalledWith('list_tracks', expect.anything());
    expect(svc.stats()?.trackCount).toBe(3);
  });
});

describe('negative cases', () => {
  it('addTrackFromPicker() rejects when pick_and_add_track fails', async () => {
    const { svc } = build(async () => {
      throw new Error('picker failed');
    });
    await expect(svc.addTrackFromPicker()).rejects.toThrow('picker failed');
    expect(svc.tracks()).toEqual([]);
  });

  it('addFolderFromPicker() rejects when pick_and_add_folder fails and does not refresh', async () => {
    const { svc, invoke } = build(async () => {
      throw new Error('folder failed');
    });
    await expect(svc.addFolderFromPicker()).rejects.toThrow('folder failed');
    expect(invoke).not.toHaveBeenCalledWith('list_tracks', expect.anything());
  });

  const refreshCases: {
    method: 'refreshTracks' | 'refreshAlbums' | 'refreshArtists' | 'refreshStats';
    cmd: string;
    signal: 'tracks' | 'albums' | 'artists' | 'stats';
    empty: unknown;
  }[] = [
    { method: 'refreshTracks', cmd: 'list_tracks', signal: 'tracks', empty: [] },
    { method: 'refreshAlbums', cmd: 'list_albums', signal: 'albums', empty: [] },
    { method: 'refreshArtists', cmd: 'list_artists', signal: 'artists', empty: [] },
    { method: 'refreshStats', cmd: 'get_library_stats', signal: 'stats', empty: null },
  ];

  for (const { method, cmd, signal, empty } of refreshCases) {
    it(`${method}() rejects when ${cmd} fails and leaves ${signal}() unchanged`, async () => {
      const { svc } = build(async () => {
        throw new Error('backend error');
      });
      await expect(svc[method]()).rejects.toThrow('backend error');
      expect(svc[signal]()).toEqual(empty);
    });
  }
});
