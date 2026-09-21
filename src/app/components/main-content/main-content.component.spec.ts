import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { MainContentComponent } from './main-content.component';

interface MainInternals {
  setMode(mode: string): void;
  setPlaylistMode(mode: 'albums' | 'songs'): void;
  toggleBrowser(): void;
  onSearchInput(event: Event): void;
  clearSearch(): void;
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [MainContentComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(MainContentComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as MainInternals,
    library: TestBed.inject(LibraryService),
    ui: TestBed.inject(UiService),
  };
}

describe('MainContentComponent', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('setMode writes to ui.libraryView', () => {
    const { cmp, ui } = setup();
    cmp.setMode('albums');
    expect(ui.libraryView()).toBe('albums');
  });

  it('setMode("queue") closes the column browser and renders the queue view', () => {
    const { fixture, cmp, ui } = setup();
    ui.columnBrowserOpen.set(true);
    cmp.setMode('queue');
    expect(ui.libraryView()).toBe('queue');
    expect(ui.columnBrowserOpen()).toBe(false);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelector('app-queue-view')).not.toBeNull();
    expect(el.querySelector('app-track-list-view')).toBeNull();
  });

  it('toggleBrowser flips columnBrowserOpen', () => {
    const { cmp, ui } = setup();
    expect(ui.columnBrowserOpen()).toBe(false);
    cmp.toggleBrowser();
    expect(ui.columnBrowserOpen()).toBe(true);
    cmp.toggleBrowser();
    expect(ui.columnBrowserOpen()).toBe(false);
  });

  it('toggleBrowser is a no-op over the queue', () => {
    const { cmp, ui } = setup();
    cmp.setMode('queue');
    cmp.toggleBrowser();
    expect(ui.columnBrowserOpen()).toBe(false);
  });

  it('playlist → queue → tracks ends with a track refresh', () => {
    const { cmp, library } = setup();
    const refresh = vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    library.activePlaylistId.set(6);
    cmp.setMode('queue');
    expect(refresh).toHaveBeenCalledTimes(1);
    cmp.setMode('tracks');
    // Queue → tracks must not render the old playlist's rows; the
    // eager refresh on playlist → queue already reloaded them.
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(ui_libraryView(cmp)).toBe('tracks');
    function ui_libraryView(c: MainInternals): string {
      void c;
      return TestBed.inject(UiService).libraryView();
    }
  });

  it('onSearchInput sets the search and debounces refreshTracks by 200ms', () => {
    const { cmp, library } = setup();
    const refresh = vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    cmp.onSearchInput({ target: { value: 'rock' } } as unknown as Event);
    expect(library.search()).toBe('rock');
    // Not yet — under the 200 ms threshold.
    expect(refresh).not.toHaveBeenCalled();
    vi.advanceTimersByTime(199);
    expect(refresh).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(refresh).toHaveBeenCalled();
  });

  it('successive onSearchInput calls cancel the prior timer', () => {
    const { cmp, library } = setup();
    const refresh = vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    cmp.onSearchInput({ target: { value: 'r' } } as unknown as Event);
    vi.advanceTimersByTime(150);
    cmp.onSearchInput({ target: { value: 'ro' } } as unknown as Event);
    vi.advanceTimersByTime(199);
    expect(refresh).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it('clearSearch clears the search slot and refreshes immediately', () => {
    const { cmp, library } = setup();
    library.setSearch('foo');
    const refresh = vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    cmp.clearSearch();
    expect(library.search()).toBe('');
    expect(refresh).toHaveBeenCalled();
  });

  describe('with a playlist open', () => {
    function openPlaylist(library: LibraryService) {
      library.playlists.set([
        {
          id: 3,
          name: 'Mix',
          kind: 'regular',
          parentId: null,
          sortOrder: 0,
          trackCount: 2,
          synced: false,
        },
      ]);
      library.activePlaylistId.set(3);
    }

    it('swaps the library toggle for the albums/songs toggle', () => {
      const { fixture, library } = setup();
      const el = fixture.nativeElement as HTMLElement;
      expect(el.querySelector('[data-testid="library-modes"]')).not.toBeNull();
      expect(el.querySelector('[data-testid="playlist-modes"]')).toBeNull();
      openPlaylist(library);
      fixture.detectChanges();
      expect(el.querySelector('[data-testid="library-modes"]')).toBeNull();
      const labels = [...el.querySelectorAll('[data-testid="playlist-modes"] button')].map((b) =>
        b.textContent?.trim(),
      );
      expect(labels).toEqual(['albums', 'songs']);
    });

    it('shows the album picker by default and the flat list on "songs"', () => {
      const { fixture, library, cmp, ui } = setup();
      openPlaylist(library);
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      expect(el.querySelector('app-playlist-album-picker')).not.toBeNull();
      expect(el.querySelector('app-track-list-view')).toBeNull();
      cmp.setPlaylistMode('songs');
      fixture.detectChanges();
      expect(ui.playlistView()).toBe('songs');
      expect(el.querySelector('app-playlist-album-picker')).toBeNull();
      expect(el.querySelector('app-track-list-view')).not.toBeNull();
    });

    it('offers the album sort only in the album view, driving the shared sort', () => {
      const { fixture, library, ui } = setup();
      library.activePlaylistId.set(42);
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      const control = () => el.querySelector('[data-testid="album-sort"]');
      expect(control()).not.toBeNull();
      const select = control()!.querySelector('select')!;
      const labels = [...select.options].map((o) => o.textContent?.trim());
      expect(labels).toEqual(['Playlist order', 'Album', 'Artist', 'Year', 'Rating', 'Date added']);

      select.value = 'year';
      select.dispatchEvent(new Event('change'));
      expect(ui.playlistAlbumSort()).toEqual({ key: 'year', descending: true });
      select.value = 'name';
      select.dispatchEvent(new Event('change'));
      expect(ui.playlistAlbumSort()).toEqual({ key: 'name', descending: false });

      fixture.detectChanges();
      const dir = control()!.querySelector('button')!;
      expect(dir.textContent?.trim()).toBe('▲');
      dir.click();
      fixture.detectChanges();
      expect(ui.playlistAlbumSort()).toEqual({ key: 'name', descending: true });
      expect(dir.textContent?.trim()).toBe('▼');

      ui.playlistView.set('songs');
      fixture.detectChanges();
      expect(control()).toBeNull();
    });

    it('keys the toolbar on the id so it never disagrees with the body', () => {
      const { fixture, library } = setup();
      // Row not loaded yet: the toggle still swaps, the name line waits.
      library.activePlaylistId.set(42);
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      expect(el.querySelector('[data-testid="playlist-modes"]')).not.toBeNull();
      expect(el.querySelector('[data-testid="active-playlist"]')).toBeNull();
      expect(el.querySelector('app-playlist-album-picker')).not.toBeNull();
    });

    it('keeps one track list mounted when leaving a playlist from "songs"', () => {
      const { fixture, library, cmp, ui } = setup();
      openPlaylist(library);
      ui.playlistView.set('songs');
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      const before = el.querySelector('app-track-list-view');
      expect(before).not.toBeNull();
      cmp.setMode('tracks');
      fixture.detectChanges();
      expect(el.querySelector('app-track-list-view')).toBe(before);
    });

    it('leaving the playlist via a library mode restores the library toggle', () => {
      const { fixture, library, cmp } = setup();
      openPlaylist(library);
      fixture.detectChanges();
      cmp.setMode('albums');
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      expect(library.activePlaylistId()).toBeNull();
      expect(el.querySelector('[data-testid="library-modes"]')).not.toBeNull();
      expect(el.querySelector('app-album-grid-view')).not.toBeNull();
    });
  });

  describe('segmented control semantics', () => {
    it('exposes the library switcher as a radio group with a checked option', () => {
      const { fixture, cmp } = setup();
      cmp.setMode('albums');
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      const group = el.querySelector('[data-testid="library-modes"]')!;
      expect(group.getAttribute('role')).toBe('radiogroup');
      expect(group.getAttribute('aria-label')).toBe('Library view');

      const radios = [...group.querySelectorAll('[role="radio"]')];
      expect(radios).toHaveLength(3);
      const checked = radios.filter((r) => r.getAttribute('aria-checked') === 'true');
      expect(checked).toHaveLength(1);
      expect(checked[0].textContent?.trim()).toBe('albums');
      // No stale aria-selected on role=radio.
      expect(radios.every((r) => r.getAttribute('aria-selected') === null)).toBe(true);
    });
  });
});
