import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { MainContentComponent } from './main-content.component';

interface MainInternals {
  setMode(mode: string): void;
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

  it('toggleBrowser flips columnBrowserOpen', () => {
    const { cmp, ui } = setup();
    expect(ui.columnBrowserOpen()).toBe(false);
    cmp.toggleBrowser();
    expect(ui.columnBrowserOpen()).toBe(true);
    cmp.toggleBrowser();
    expect(ui.columnBrowserOpen()).toBe(false);
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
});
