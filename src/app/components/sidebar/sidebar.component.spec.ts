import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { LibraryService, Playlist } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SidebarComponent, buildPlaylistTree } from './sidebar.component';

interface SidebarInternals {
  setView(v: string): void;
  isActive(v: string): boolean;
  onPlaylistClick(p: Playlist): void;
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
    cmp.onPlaylistClick(folder);
    fixture.detectChanges();
    expect(cmp.isExpanded(folder)).toBe(true);
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Rock', 'Metal', 'Chill', 'Orphan']);
    expect(library.activePlaylistId()).toBeNull();
  });

  it('clicking a playlist opens it in the tracks view and deactivates library views', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    ui.libraryView.set('albums');
    ui.columnBrowserOpen.set(true);
    cmp.onPlaylistClick(library.playlists().find((p) => p.id === 6)!);
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

  it('renders the All Songs / Artists / Albums / Genres buttons', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    for (const label of ['All Songs', 'Artists', 'Albums', 'Genres']) {
      expect(text).toContain(label);
    }
  });
});
