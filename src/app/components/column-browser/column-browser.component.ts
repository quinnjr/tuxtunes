import { Component, OnInit, computed, effect, inject, signal } from '@angular/core';
import {
  DistinctColumn,
  DistinctValue,
  LibraryService,
  TrackFilters,
} from '../../services/library.service';

interface PaneState {
  column: DistinctColumn;
  values: DistinctValue[];
}

@Component({
  selector: 'app-column-browser',
  imports: [],
  templateUrl: './column-browser.component.html',
})
export class ColumnBrowserComponent implements OnInit {
  protected readonly library = inject(LibraryService);

  /** Three fixed panes for v1. Configurable column choice is deferred. */
  protected readonly panes = signal<PaneState[]>([
    { column: 'genre', values: [] },
    { column: 'artist', values: [] },
    { column: 'album', values: [] },
  ]);

  /** Pane-search inputs, one per pane index. */
  protected readonly paneSearch = signal<string[]>(['', '', '']);

  /**
   * Filtered+rendered values per pane, derived from `panes` + `paneSearch`.
   * The pane-local search is a substring filter applied client-side over
   * the already-fetched distinct values; selection counts come from the
   * server query so they remain accurate under cross-column filters.
   */
  protected readonly visiblePanes = computed(this.#computeVisiblePanes.bind(this));

  constructor() {
    effect(() => {
      // Re-run distinct queries whenever the cross-column filter set
      // changes. The filters signal is the single source of truth.
      void this.library.filters();
      void this.refreshAll();
    });
  }

  ngOnInit(): void {
    void this.refreshAll();
  }

  #computeVisiblePanes(): { column: DistinctColumn; values: DistinctValue[] }[] {
    const panes = this.panes();
    const searches = this.paneSearch();
    return panes.map((p, i) => {
      const q = (searches[i] ?? '').trim().toLowerCase();
      if (q === '') return p;
      return {
        column: p.column,
        values: p.values.filter((v) => v.value.toLowerCase().includes(q)),
      };
    });
  }

  protected async refreshAll(): Promise<void> {
    const panes = this.panes();
    const next: PaneState[] = await Promise.all(
      panes.map(async (p) => ({
        column: p.column,
        values: await this.library.getDistinct(p.column),
      })),
    );
    this.panes.set(next);
  }

  protected paneTitle(column: DistinctColumn): string {
    return column[0].toUpperCase() + column.slice(1) + 's';
  }

  protected isSelected(column: DistinctColumn, value: string): boolean {
    return this.activeValuesFor(column).includes(value);
  }

  protected activeValuesFor(column: DistinctColumn): string[] {
    const f = this.library.filters();
    if (column === 'genre') return f.genres;
    if (column === 'artist') return f.artists;
    return f.albums;
  }

  /**
   * Toggle membership of `value` in column's active filter slot. Multi-
   * select is union semantics: shift/ctrl-click adds; plain click
   * collapses to that single value (or clears if it was the only one
   * already selected).
   */
  protected async toggle(column: DistinctColumn, value: string, multi: boolean): Promise<void> {
    const current = this.activeValuesFor(column);
    let next: string[];
    if (multi) {
      next = current.includes(value) ? current.filter((v) => v !== value) : [...current, value];
    } else if (current.length === 1 && current[0] === value) {
      next = [];
    } else {
      next = [value];
    }
    this.library.filters.update((f) => writeColumn(f, column, next));
    await this.library.refreshTracks();
  }

  protected onValueClick(column: DistinctColumn, value: string, event: MouseEvent): void {
    void this.toggle(column, value, event.shiftKey || event.ctrlKey || event.metaKey);
  }

  protected clearPane(column: DistinctColumn): void {
    this.library.filters.update((f) => writeColumn(f, column, []));
    void this.library.refreshTracks();
  }

  protected onPaneSearchInput(index: number, event: Event): void {
    const value = (event.target as HTMLInputElement).value;
    this.paneSearch.update((arr) => arr.map((v, i) => (i === index ? value : v)));
  }

  protected paneSearchValue(index: number): string {
    return this.paneSearch()[index] ?? '';
  }

  protected trackByValue(_index: number, v: DistinctValue): string {
    return v.value;
  }
}

function writeColumn(
  filters: TrackFilters,
  column: DistinctColumn,
  values: string[],
): TrackFilters {
  if (column === 'genre') return { ...filters, genres: values };
  if (column === 'artist') return { ...filters, artists: values };
  return { ...filters, albums: values };
}
