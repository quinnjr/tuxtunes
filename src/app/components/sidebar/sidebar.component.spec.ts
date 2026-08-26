import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { LibraryService, Playlist } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SidebarComponent, buildPlaylistTree } from './sidebar.component';

interface SidebarInternals {
  setView(v: string): void;
  isActive(v: string): boolean;
  onPlaylistClick(node: { playlist: Playlist; children: unknown[] }): void;
  isFolder(node: { playlist: Playlist; children: unknown[] }): boolean;
  onPlaylistContextMenu(node: { playlist: Playlist; children: unknown[] }, event: MouseEvent): void;
  isExpanded(p: Playlist): boolean;
}

const RAW_PLAYLISTS = [
  { id: 5, name: 'Rock', kind: 'folder', parent_id: null, sort_order: 1, cached_track_count: 0 },
  { id: 6, name: 'Metal', kind: 'regular', parent_id: 5, sort_order: 0, cached_track_count: 12 },
  { id: 7, name: 'Chill', kind: 'smart', parent_id: null, sort_order: 2, cached_track_count: null },
  { id: 8, name: 'Orphan', kind: 'regular', parent_id: 999, sort_order: 3, cached_track_count: 1 },
];

function pl(over: Partial<Playlist>): Playlist {
  return {
    id: 1,
    name: 'p',
    kind: 'regular',
    parentId: null,
    sortOrder: 0,
    trackCount: 0,
    ...over,
  };
}

function setup(playlists: unknown[] = []) {
  const stub = tauriStub(async (cmd) => {
    if (cmd === 'list_playlists') return playlists;
    return defaultInvoke(cmd);
  });
  TestBed.configureTestingModule({
    imports: [SidebarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SidebarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as SidebarInternals,
    ui: TestBed.inject(UiService),
    library: TestBed.inject(LibraryService),
    stub,
  };
}

async function settle(fixture: { detectChanges(): void }): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  fixture.detectChanges();
}

describe('buildPlaylistTree', () => {
  it('nests children under folders and keeps sibling order', () => {
    const tree = buildPlaylistTree([
      pl({ id: 1, name: 'A', kind: 'folder' }),
      pl({ id: 2, name: 'A1', parentId: 1 }),
      pl({ id: 3, name: 'B' }),
      pl({ id: 4, name: 'A2', parentId: 1 }),
    ]);
    expect(tree.map((n) => n.playlist.name)).toEqual(['A', 'B']);
    expect(tree[0].children.map((n) => n.playlist.name)).toEqual(['A1', 'A2']);
  });

  it('promotes playlists with a missing parent to the root', () => {
    const tree = buildPlaylistTree([pl({ id: 1, name: 'Lost', parentId: 42 })]);
    expect(tree.map((n) => n.playlist.name)).toEqual(['Lost']);
  });

  it('does not loop on a self-parented row', () => {
    const tree = buildPlaylistTree([pl({ id: 1, name: 'Self', parentId: 1, kind: 'folder' })]);
    expect(tree).toHaveLength(1);
    expect(tree[0].children).toHaveLength(0);
  });

  it('breaks a parent_id cycle by promoting its members to root, keeping non-member children in place', () => {
    // A.parent = B, B.parent = A, C.parent = A. Without the fix, A and
    // B are only ever linked as each other's child and never reach the
    // roots array — the whole trio (including C) silently vanishes.
    const tree = buildPlaylistTree([
      pl({ id: 1, name: 'A', parentId: 2, kind: 'folder' }),
      pl({ id: 2, name: 'B', parentId: 1, kind: 'folder' }),
      pl({ id: 3, name: 'C', parentId: 1 }),
    ]);
    expect(tree.map((n) => n.playlist.name).sort()).toEqual(['A', 'B']);
    const a = tree.find((n) => n.playlist.name === 'A')!;
    const b = tree.find((n) => n.playlist.name === 'B')!;
    expect(a.children.map((n) => n.playlist.name)).toEqual(['C']);
    expect(b.children).toHaveLength(0);
  });
});

describe('SidebarComponent', () => {
  it('setView("genres") opens the column browser and keeps libraryView=tracks', () => {
    const { cmp, ui } = setup();
    cmp.setView('genres');
    expect(ui.libraryView()).toBe('tracks');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('setView() for other views switches libraryView without touching columnBrowser', () => {
    const { cmp, ui } = setup();
    ui.columnBrowserOpen.set(true);
    cmp.setView('albums');
    expect(ui.libraryView()).toBe('albums');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('isActive() resolves the genres pseudo-view via the column browser flag', () => {
    const { cmp, ui } = setup();
    ui.libraryView.set('tracks');
    ui.columnBrowserOpen.set(true);
    expect(cmp.isActive('genres')).toBe(true);
    expect(cmp.isActive('tracks')).toBe(false);
    ui.columnBrowserOpen.set(false);
    expect(cmp.isActive('genres')).toBe(false);
    expect(cmp.isActive('tracks')).toBe(true);
  });

  it('lists playlists from list_playlists on init, folders collapsed', async () => {
    const { fixture, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    expect(stub.invoke).toHaveBeenCalledWith('list_playlists');
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Rock', 'Chill', 'Orphan']);
    expect(el.textContent).not.toContain('No playlists yet');
  });

  it('clicking a folder expands it to reveal children', async () => {
    const { fixture, cmp, library } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const folder = library.playlists().find((p) => p.id === 5)!;
    cmp.onPlaylistClick({ playlist: folder, children: [{}] });
    fixture.detectChanges();
    expect(cmp.isExpanded(folder)).toBe(true);
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Rock', 'Metal', 'Chill', 'Orphan']);
    expect(library.activePlaylistId()).toBeNull();
  });

  it("a 'regular' node with children is treated as a folder (old syncs stored folders that way)", async () => {
    const { fixture, cmp, library, stub } = setup([
      {
        id: 1,
        name: 'Genre',
        kind: 'regular',
        parent_id: null,
        sort_order: 0,
        cached_track_count: 900,
      },
      { id: 2, name: 'Band', kind: 'regular', parent_id: 1, sort_order: 0, cached_track_count: 9 },
    ]);
    await settle(fixture);
    const genre = library.playlists().find((p) => p.id === 1)!;
    const node = { playlist: genre, children: [{}] };
    expect(cmp.isFolder(node)).toBe(true);
    stub.invoke.mockClear();
    cmp.onPlaylistClick(node);
    await settle(fixture);
    expect(cmp.isExpanded(genre)).toBe(true);
    expect(library.activePlaylistId()).toBeNull();
    expect(stub.invoke).not.toHaveBeenCalledWith('open_playlist', expect.anything());
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Genre', 'Band']);
  });

  it('clicking a playlist opens it in the tracks view and deactivates library views', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    ui.libraryView.set('albums');
    ui.columnBrowserOpen.set(true);
    cmp.onPlaylistClick({ playlist: library.playlists().find((p) => p.id === 6)!, children: [] });
    await settle(fixture);
    expect(ui.libraryView()).toBe('tracks');
    expect(ui.columnBrowserOpen()).toBe(false);
    expect(library.activePlaylistId()).toBe(6);
    expect(stub.invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 6 });
    expect(cmp.isActive('tracks')).toBe(false);
  });

  it('setView() leaves the active playlist and reloads the library list', async () => {
    const { fixture, cmp, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    library.activePlaylistId.set(6);
    stub.invoke.mockClear();
    cmp.setView('tracks');
    await settle(fixture);
    expect(library.activePlaylistId()).toBeNull();
    expect(stub.invoke).toHaveBeenCalledWith('list_tracks', expect.anything());
    expect(cmp.isActive('tracks')).toBe(true);
  });

  it('right-click offers Edit + Delete for smart playlists, Remove for synced, nothing for folders', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    const ev = new MouseEvent('contextmenu');
    const smart = library.playlists().find((p) => p.id === 7)!;
    cmp.onPlaylistContextMenu({ playlist: smart, children: [] }, ev);
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items.map((i) => i.label)).toEqual(['Edit Smart Playlist…', 'Delete']);
    await items[0].action?.();
    expect(ui.smartEditor()).toEqual({ playlistId: 7 });
    library.activePlaylistId.set(7);
    await items[1].action?.();
    expect(stub.invoke).toHaveBeenCalledWith('delete_playlist', { playlistId: 7 });
    expect(library.activePlaylistId()).toBeNull();

    show.mockClear();
    const regular = library.playlists().find((p) => p.id === 6)!;
    cmp.onPlaylistContextMenu({ playlist: regular, children: [] }, ev);
    const items2 = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items2.map((i) => i.label)).toEqual(['Remove until next sync']);

    show.mockClear();
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 5)!, children: [{}] },
      ev,
    );
    expect(show).not.toHaveBeenCalled();
  });

  it('renders the All Songs / Artists / Albums / Genres buttons', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    for (const label of ['All Songs', 'Artists', 'Albums', 'Genres']) {
      expect(text).toContain(label);
    }
  });
});
