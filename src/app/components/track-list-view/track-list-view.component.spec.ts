import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { LibraryService, type SortColumn } from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { TauriService } from '../../services/tauri.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { TrackListViewComponent } from './track-list-view.component';

interface ListInternals {
  visibleColumnIds: {
    (): SortColumn[];
    set(v: SortColumn[]): void;
    update(fn: (v: SortColumn[]) => SortColumn[]): void;
  };
  visibleColumns(): { id: SortColumn; label: string; format(t: TrackRow): string }[];
  selection: { (): Set<number>; set(v: Set<number>): void };
  pickerOpen: { (): boolean };
  isCurrent(t: TrackRow): boolean;
  isSelected(t: TrackRow): boolean;
  isSortColumn(id: SortColumn): boolean;
  sortIndicator(id: SortColumn): string;
  cycleSort(id: SortColumn): Promise<void>;
  play(t: TrackRow): Promise<void>;
  onRowClick(index: number, t: TrackRow, event: MouseEvent): void;
  onRowContextMenu(t: TrackRow, event: MouseEvent): void;
  togglePicker(event: MouseEvent): void;
  toggleColumn(id: SortColumn): void;
  isColumnVisible(id: SortColumn): boolean;
  closePicker(): void;
}

const TRACK = (id: number, overrides: Partial<TrackRow> = {}): TrackRow => ({
  id,
  title: `Track ${id}`,
  artist: 'A',
  album: 'Al',
  durationMs: 60_000,
  filePath: `/tmp/${id}.flac`,
  sampleRate: 44_100,
  bitDepth: 16,
  kind: 'flac',
  playCount: 0,
  skipCount: 0,
  ...overrides,
});

function setup(
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> = async () => [],
) {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [TrackListViewComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(TrackListViewComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as ListInternals,
    library: TestBed.inject(LibraryService),
    playback: TestBed.inject(PlaybackService),
    ctx: TestBed.inject(ContextMenuService),
    tauri: TestBed.inject(TauriService),
    invoke: stub.invoke,
  };
}

describe('TrackListViewComponent', () => {
  it('refreshes tracks on init', () => {
    const { invoke } = setup();
    expect(invoke).toHaveBeenCalledWith(
      'list_tracks',
      expect.objectContaining({ limit: 500, offset: 0 }),
    );
  });

  it('isCurrent reflects PlaybackService.currentTrackId', () => {
    const { cmp, playback } = setup();
    const t = TRACK(7);
    expect(cmp.isCurrent(t)).toBe(false);
    playback.currentTrackId.set(7);
    expect(cmp.isCurrent(t)).toBe(true);
  });

  it('cycleSort flips direction and refreshes via LibraryService', async () => {
    const { cmp, library } = setup();
    const spy = vi.spyOn(library, 'cycleSort').mockResolvedValue();
    await cmp.cycleSort('title');
    expect(spy).toHaveBeenCalledWith('title');
  });

  it('play() forwards to PlaybackService.play', async () => {
    const { cmp, playback } = setup();
    const spy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.play(TRACK(3));
    expect(spy).toHaveBeenCalledWith(3);
  });

  it('isSortColumn / sortIndicator reflect LibraryService.sort', () => {
    const { cmp, library } = setup();
    library.sort.set({ column: 'title', descending: true });
    expect(cmp.isSortColumn('title')).toBe(true);
    expect(cmp.sortIndicator('title')).toBe('▼');
    library.sort.set({ column: 'title', descending: false });
    expect(cmp.sortIndicator('title')).toBe('▲');
    expect(cmp.sortIndicator('album')).toBe('');
  });

  it('plain row click replaces selection with the clicked row', () => {
    const { cmp, library } = setup();
    library.tracks.set([TRACK(1), TRACK(2), TRACK(3)]);
    cmp.onRowClick(1, TRACK(2), { ctrlKey: false, metaKey: false, shiftKey: false } as MouseEvent);
    expect([...cmp.selection()]).toEqual([2]);
  });

  it('ctrl-click toggles + moves anchor', () => {
    const { cmp, library } = setup();
    library.tracks.set([TRACK(1), TRACK(2), TRACK(3)]);
    cmp.onRowClick(0, TRACK(1), { ctrlKey: true, metaKey: false, shiftKey: false } as MouseEvent);
    cmp.onRowClick(2, TRACK(3), { ctrlKey: true, metaKey: false, shiftKey: false } as MouseEvent);
    expect(cmp.selection().size).toBe(2);
    cmp.onRowClick(2, TRACK(3), { ctrlKey: true, metaKey: false, shiftKey: false } as MouseEvent);
    expect(cmp.selection().has(3)).toBe(false);
  });

  it('shift-click range-selects from the anchor', () => {
    const { cmp, library } = setup();
    library.tracks.set([TRACK(1), TRACK(2), TRACK(3), TRACK(4)]);
    cmp.onRowClick(0, TRACK(1), { ctrlKey: false, metaKey: false, shiftKey: false } as MouseEvent);
    cmp.onRowClick(3, TRACK(4), { ctrlKey: false, metaKey: false, shiftKey: true } as MouseEvent);
    expect(cmp.selection().size).toBe(4);
  });

  it('right-click shows the context menu with single-target labels', () => {
    const { cmp, ctx, library } = setup();
    library.tracks.set([TRACK(1)]);
    const showSpy = vi.spyOn(ctx, 'show');
    cmp.onRowContextMenu(TRACK(1), {
      preventDefault: vi.fn(),
      clientX: 0,
      clientY: 0,
    } as unknown as MouseEvent);
    expect(showSpy).toHaveBeenCalled();
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items[0].label).toBe('Play');
  });

  it('right-click while in a multi-selection scopes the menu to the selection', () => {
    const { cmp, ctx, library } = setup();
    library.tracks.set([TRACK(1), TRACK(2)]);
    cmp.selection.set(new Set([1, 2]));
    const showSpy = vi.spyOn(ctx, 'show');
    cmp.onRowContextMenu(TRACK(2), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items[0].label).toBe('Play first (2 selected)');
  });

  it('context-menu actions: play, enqueue, play-next, show-in-files', async () => {
    const { cmp, ctx, library, playback, invoke } = setup();
    library.tracks.set([TRACK(1)]);
    const showSpy = vi.spyOn(ctx, 'show');
    cmp.onRowContextMenu(TRACK(1), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    const enqueueSpy = vi.spyOn(playback, 'enqueue');
    const playNextSpy = vi.spyOn(playback, 'playNext');
    items[0].action?.();
    items[1].action?.();
    items[2].action?.();
    await items[4].action?.();
    expect(playSpy).toHaveBeenCalledWith(1);
    expect(enqueueSpy).toHaveBeenCalled();
    expect(playNextSpy).toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith('show_in_files', { trackId: 1 });
  });

  it('context-menu remove + trash actions', async () => {
    const { cmp, ctx, library, invoke } = setup();
    library.tracks.set([TRACK(1)]);
    const showSpy = vi.spyOn(ctx, 'show');
    cmp.onRowContextMenu(TRACK(1), { preventDefault: vi.fn() } as unknown as MouseEvent);
    const items = (showSpy.mock.calls[0][1] ?? []) as ContextMenuItem[];
    await items[6].action?.();
    expect(invoke).toHaveBeenCalledWith('remove_track', { trackId: 1 });
    await items[7].action?.();
    expect(invoke).toHaveBeenCalledWith('trash_track', { trackId: 1 });
  });

  it('column picker opens, toggles columns, then closes', () => {
    const { cmp } = setup();
    expect(cmp.pickerOpen()).toBe(false);
    cmp.togglePicker({ stopPropagation: vi.fn() } as unknown as MouseEvent);
    expect(cmp.pickerOpen()).toBe(true);

    expect(cmp.isColumnVisible('artist')).toBe(true);
    cmp.toggleColumn('artist');
    expect(cmp.isColumnVisible('artist')).toBe(false);
    cmp.toggleColumn('artist');
    expect(cmp.isColumnVisible('artist')).toBe(true);

    cmp.closePicker();
    expect(cmp.pickerOpen()).toBe(false);
  });

  it('visibleColumns() filters out unknown ids defensively', () => {
    const { cmp } = setup();
    cmp.visibleColumnIds.set(['title', 'bogus' as SortColumn]);
    expect(cmp.visibleColumns().map((c) => c.id)).toEqual(['title']);
  });

  it('column format helpers cover every column', () => {
    const { cmp } = setup();
    cmp.visibleColumnIds.set([
      'title',
      'artist',
      'album',
      'duration_ms',
      'play_count',
      'sample_rate',
      'kind',
    ]);
    const cols = cmp.visibleColumns();
    const get = (id: string) => cols.find((c) => c.id === id);

    // Title: identity.
    expect(get('title')?.format(TRACK(1))).toBe('Track 1');
    // Album / artist / kind null fallbacks render as empty string.
    expect(get('album')?.format(TRACK(1, { album: 'Abbey Road' }))).toBe('Abbey Road');
    expect(get('album')?.format(TRACK(1, { album: null }))).toBe('');
    expect(get('artist')?.format(TRACK(1, { artist: null }))).toBe('');
    expect(get('kind')?.format(TRACK(1, { kind: 'flac' }))).toBe('flac');
    expect(get('kind')?.format(TRACK(1, { kind: null }))).toBe('');
    // Sample rate formatting.
    expect(get('sample_rate')?.format(TRACK(1, { sampleRate: 96_000 }))).toBe('96.0k');
    expect(get('sample_rate')?.format(TRACK(1, { sampleRate: null }))).toBe('');
    // Play count + duration.
    expect(get('play_count')?.format(TRACK(1, { playCount: 7 }))).toBe('7');
    expect(get('duration_ms')?.format(TRACK(1, { durationMs: 65_000 }))).toBe('1:05');
  });

  it('trackById returns the row id', () => {
    const { fixture } = setup();
    const cmp = fixture.componentInstance as unknown as {
      trackById(i: number, t: TrackRow): number;
    };
    expect(cmp.trackById(0, TRACK(42))).toBe(42);
  });

  it('isSelected reflects the selection signal', () => {
    const { cmp } = setup();
    cmp.selection.set(new Set([5]));
    expect(cmp.isSelected(TRACK(5))).toBe(true);
    expect(cmp.isSelected(TRACK(6))).toBe(false);
  });
});
