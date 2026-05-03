import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import {
  LibraryService,
  type DistinctColumn,
  type DistinctValue,
} from '../../services/library.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { ColumnBrowserComponent } from './column-browser.component';

interface BrowserInternals {
  panes: { (): { column: DistinctColumn; values: DistinctValue[] }[] };
  paneSearch: { (): string[]; set(v: string[]): void };
  visiblePanes(): { column: DistinctColumn; values: DistinctValue[] }[];
  paneTitle(column: DistinctColumn): string;
  isSelected(column: DistinctColumn, value: string): boolean;
  activeValuesFor(column: DistinctColumn): string[];
  toggle(column: DistinctColumn, value: string, multi: boolean): Promise<void>;
  onValueClick(column: DistinctColumn, value: string, event: MouseEvent): void;
  clearPane(column: DistinctColumn): void;
  onPaneSearchInput(index: number, event: Event): void;
  paneSearchValue(index: number): string;
  trackByValue(i: number, v: DistinctValue): string;
  refreshAll(): Promise<void>;
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [ColumnBrowserComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(ColumnBrowserComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as BrowserInternals,
    library: TestBed.inject(LibraryService),
  };
}

describe('ColumnBrowserComponent', () => {
  it('paneTitle capitalizes and pluralizes the column name', () => {
    const { cmp } = setup();
    expect(cmp.paneTitle('genre')).toBe('Genres');
    expect(cmp.paneTitle('artist')).toBe('Artists');
    expect(cmp.paneTitle('album')).toBe('Albums');
  });

  it('activeValuesFor maps each column to its filter slot', () => {
    const { cmp, library } = setup();
    library.filters.set({ genres: ['G'], artists: ['A'], albums: ['B'], search: null });
    expect(cmp.activeValuesFor('genre')).toEqual(['G']);
    expect(cmp.activeValuesFor('artist')).toEqual(['A']);
    expect(cmp.activeValuesFor('album')).toEqual(['B']);
  });

  it('isSelected reflects filter membership', () => {
    const { cmp, library } = setup();
    library.filters.update((f) => ({ ...f, genres: ['Rock'] }));
    expect(cmp.isSelected('genre', 'Rock')).toBe(true);
    expect(cmp.isSelected('genre', 'Jazz')).toBe(false);
  });

  it('plain toggle replaces selection; same-value toggle clears', async () => {
    const { cmp, library } = setup();
    vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    await cmp.toggle('genre', 'Rock', false);
    expect(library.filters().genres).toEqual(['Rock']);
    await cmp.toggle('genre', 'Rock', false);
    expect(library.filters().genres).toEqual([]);
  });

  it('multi-select toggle adds and removes union members', async () => {
    const { cmp, library } = setup();
    vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    await cmp.toggle('artist', 'A', true);
    await cmp.toggle('artist', 'B', true);
    expect(library.filters().artists).toEqual(['A', 'B']);
    await cmp.toggle('artist', 'A', true);
    expect(library.filters().artists).toEqual(['B']);
  });

  it('onValueClick treats shift/ctrl/meta as multi-select', () => {
    const { cmp, library } = setup();
    vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    cmp.onValueClick('genre', 'Rock', { shiftKey: true } as unknown as MouseEvent);
    expect(library.filters().genres).toEqual(['Rock']);
  });

  it('clearPane resets that column’s filter slot', () => {
    const { cmp, library } = setup();
    library.filters.update((f) => ({ ...f, genres: ['Rock', 'Jazz'] }));
    vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    cmp.clearPane('genre');
    expect(library.filters().genres).toEqual([]);
  });

  it('onPaneSearchInput writes the input value into paneSearch[index]', () => {
    const { cmp } = setup();
    cmp.onPaneSearchInput(1, { target: { value: 'roc' } } as unknown as Event);
    expect(cmp.paneSearchValue(1)).toBe('roc');
  });

  it('visiblePanes filters values by per-pane substring search', () => {
    const { cmp } = setup();
    // Manually seed pane values then ask for visiblePanes.
    (cmp as unknown as { panes: { set: (v: unknown) => void } }).panes.set([
      {
        column: 'genre',
        values: [
          { value: 'Rock', count: 1 },
          { value: 'Jazz', count: 1 },
        ],
      },
      { column: 'artist', values: [] },
      { column: 'album', values: [] },
    ]);
    cmp.paneSearch.set(['roc', '', '']);
    const visible = cmp.visiblePanes();
    expect(visible[0].values.map((v) => v.value)).toEqual(['Rock']);
  });

  it('trackByValue uses the value text', () => {
    const { cmp } = setup();
    expect(cmp.trackByValue(0, { value: 'X', count: 1 })).toBe('X');
  });

  it('toggle on the album column writes through writeColumn’s fallback arm', async () => {
    const { cmp, library } = setup();
    vi.spyOn(library, 'refreshTracks').mockResolvedValue();
    await cmp.toggle('album', 'Abbey Road', false);
    expect(library.filters().albums).toEqual(['Abbey Road']);
  });

  it('refreshAll fetches distinct values for every pane', async () => {
    const { cmp, library } = setup();
    const spy = vi
      .spyOn(library, 'getDistinct')
      .mockResolvedValue([{ value: 'X', count: 1 }] as DistinctValue[]);
    await cmp.refreshAll();
    // 3 panes × 1 call. The constructor effect + ngOnInit may have
    // called refreshAll already, so just assert the spy fired at
    // least three times in this manual call.
    expect(spy.mock.calls.length).toBeGreaterThanOrEqual(3);
  });
});
