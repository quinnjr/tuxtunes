import { ScrollingModule } from '@angular/cdk/scrolling';
import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';
import { LibraryService, SortColumn } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { TauriService } from '../../services/tauri.service';
import { formatMmSs } from '../../utils/time';

interface Column {
  id: SortColumn;
  label: string;
  /** Tailwind width class. Falls back to flex-1 when omitted. */
  widthClass?: string;
  /** Right-align numeric columns. */
  numeric?: boolean;
  format(t: TrackRow): string;
}

const ALL_COLUMNS: Column[] = [
  { id: 'title', label: 'Title', format: (t) => t.title },
  { id: 'artist', label: 'Artist', widthClass: 'w-48', format: (t) => t.artist ?? '' },
  { id: 'album', label: 'Album', widthClass: 'w-48', format: (t) => t.album ?? '' },
  {
    id: 'duration_ms',
    label: 'Time',
    widthClass: 'w-16',
    numeric: true,
    format: (t) => formatMmSs(t.durationMs),
  },
  {
    id: 'play_count',
    label: 'Plays',
    widthClass: 'w-16',
    numeric: true,
    format: (t) => String(t.playCount),
  },
  {
    id: 'sample_rate',
    label: 'Sample',
    widthClass: 'w-20',
    numeric: true,
    format: (t) => (t.sampleRate ? `${(t.sampleRate / 1000).toFixed(1)}k` : ''),
  },
  {
    id: 'kind',
    label: 'Kind',
    widthClass: 'w-20',
    format: (t) => t.kind ?? '',
  },
];

@Component({
  selector: 'app-track-list-view',
  imports: [ScrollingModule],
  templateUrl: './track-list-view.component.html',
})
export class TrackListViewComponent implements OnInit {
  protected readonly library = inject(LibraryService);
  protected readonly playback = inject(PlaybackService);
  private readonly tauri = inject(TauriService);
  private readonly ctx = inject(ContextMenuService);

  /** All columns the user can choose from. */
  protected readonly allColumns = ALL_COLUMNS;

  /** Currently-shown column ids. The picker writes here. */
  protected readonly visibleColumnIds = signal<SortColumn[]>([
    'title',
    'artist',
    'album',
    'duration_ms',
    'play_count',
  ]);

  /** Resolved column descriptors in display order. */
  protected readonly visibleColumns = computed(this.#computeVisibleColumns.bind(this));

  /** Multi-selected track ids (Set keyed by id). */
  protected readonly selection = signal<Set<number>>(new Set<number>());

  /** Anchor row index for shift-click range selection. */
  private anchorIndex: number | null = null;

  /** Column-picker [⚙] popover. */
  protected readonly pickerOpen = signal(false);

  ngOnInit(): void {
    void this.library.refreshTracks();
  }

  #computeVisibleColumns(): Column[] {
    const ids = this.visibleColumnIds();
    return ids
      .map((id) => ALL_COLUMNS.find((c) => c.id === id))
      .filter((c): c is Column => c !== undefined);
  }

  protected trackById = (_: number, row: TrackRow): number => row.id;

  protected isCurrent(t: TrackRow): boolean {
    return this.playback.currentTrackId() === t.id;
  }

  protected isSelected(t: TrackRow): boolean {
    return this.selection().has(t.id);
  }

  protected isSortColumn(id: SortColumn): boolean {
    return this.library.sort().column === id;
  }

  protected sortIndicator(id: SortColumn): string {
    if (!this.isSortColumn(id)) return '';
    return this.library.sort().descending ? '▼' : '▲';
  }

  protected async play(t: TrackRow): Promise<void> {
    await this.playback.play(t.id);
  }

  protected async cycleSort(id: SortColumn): Promise<void> {
    await this.library.cycleSort(id);
  }

  /**
   * Standard list-control selection semantics:
   * - plain click: replace with this row
   * - ctrl/cmd-click: toggle this row, anchor moves
   * - shift-click: range select from anchor to this row
   */
  protected onRowClick(index: number, t: TrackRow, event: MouseEvent): void {
    const isMulti = event.ctrlKey || event.metaKey;
    if (event.shiftKey && this.anchorIndex !== null) {
      const tracks = this.library.tracks();
      const [a, b] = [this.anchorIndex, index].sort((x, y) => x - y);
      const next = new Set(this.selection());
      for (let i = a; i <= b; i += 1) next.add(tracks[i].id);
      this.selection.set(next);
      return;
    }
    if (isMulti) {
      const next = new Set(this.selection());
      if (next.has(t.id)) next.delete(t.id);
      else next.add(t.id);
      this.selection.set(next);
      this.anchorIndex = index;
      return;
    }
    this.selection.set(new Set([t.id]));
    this.anchorIndex = index;
  }

  /**
   * Right-click resolves a context-menu item set scoped to the
   * effective selection — the clicked row plus any prior multi-selection
   * if it included the row, otherwise just the clicked row.
   */
  protected onRowContextMenu(t: TrackRow, event: MouseEvent): void {
    let targets: TrackRow[];
    if (this.selection().has(t.id)) {
      targets = this.library.tracks().filter((r) => this.selection().has(r.id));
    } else {
      targets = [t];
      this.selection.set(new Set([t.id]));
    }
    this.ctx.show(event, this.buildMenu(targets));
  }

  private buildMenu(targets: TrackRow[]): ContextMenuItem[] {
    const single = targets.length === 1;
    const t = targets[0];
    return [
      {
        label: single ? 'Play' : `Play first (${targets.length} selected)`,
        action: () => void this.play(t),
      },
      {
        label: single ? 'Add to queue' : `Add ${targets.length} to queue`,
        action: () => {
          for (const target of targets) this.playback.enqueue(target);
        },
      },
      {
        label: single ? 'Play next' : `Play ${targets.length} next`,
        action: () => {
          for (const target of [...targets].reverse()) this.playback.playNext(target);
        },
      },
      { label: '---' },
      {
        label: 'Show in Files',
        disabled: !single,
        action: async () => {
          await this.tauri.invoke('show_in_files', { trackId: t.id });
        },
      },
      { label: '---' },
      {
        label: single ? 'Remove from Library' : `Remove ${targets.length} from Library`,
        destructive: true,
        action: async () => {
          for (const target of targets) {
            await this.tauri.invoke('remove_track', { trackId: target.id });
          }
          this.selection.set(new Set());
          await this.library.refreshTracks();
          await this.library.refreshStats();
        },
      },
      {
        label: single ? 'Move to Trash' : `Move ${targets.length} to Trash`,
        destructive: true,
        action: async () => {
          for (const target of targets) {
            await this.tauri.invoke('trash_track', { trackId: target.id });
          }
          this.selection.set(new Set());
          await this.library.refreshTracks();
          await this.library.refreshStats();
        },
      },
    ];
  }

  protected togglePicker(event: MouseEvent): void {
    event.stopPropagation();
    this.pickerOpen.update((v) => !v);
  }

  protected toggleColumn(id: SortColumn): void {
    this.visibleColumnIds.update((ids) =>
      ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id],
    );
  }

  protected isColumnVisible(id: SortColumn): boolean {
    return this.visibleColumnIds().includes(id);
  }

  protected closePicker(): void {
    this.pickerOpen.set(false);
  }
}
