import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LeafCondition, SmartRule, defaultRule } from '../../models/smart';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SmartPlaylistEditorComponent } from './smart-playlist-editor.component';

interface EditorInternals {
  name: { (): string; set(v: string): void };
  rule: { (): SmartRule; set(v: SmartRule): void };
  rows(): LeafCondition[];
  matchCount(): number | null;
  editingId(): number | null;
  canSave(): boolean;
  addRow(): void;
  removeRow(i: number): void;
  setField(i: number, field: string): void;
  setOp(i: number, op: string): void;
  setText(i: number, text: string): void;
  setRange(i: number, part: 'from' | 'to', raw: string): void;
  setRelative(i: number, part: 'n' | 'unit', raw: string): void;
  setMatchAll(all: boolean): void;
  toggleLimit(on: boolean): void;
  setLimitValue(raw: string): void;
  widget(row: LeafCondition): string;
  save(): Promise<void>;
  close(): void;
}

function setup(invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [SmartPlaylistEditorComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SmartPlaylistEditorComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as EditorInternals,
    ui: TestBed.inject(UiService),
    library: TestBed.inject(LibraryService),
    stub,
  };
}

async function settle(fixture: { detectChanges(): void }): Promise<void> {
  for (let i = 0; i < 6; i += 1) await Promise.resolve();
  fixture.detectChanges();
}

describe('SmartPlaylistEditorComponent', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('opens blank for a new playlist and previews the rule after a debounce', async () => {
    const { fixture, cmp, ui, stub } = setup(async (cmd) => {
      if (cmd === 'preview_smart_rule') return 42;
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: null });
    fixture.detectChanges();
    await settle(fixture);
    expect(cmp.editingId()).toBeNull();
    expect(cmp.name()).toBe('');
    expect(cmp.rows()).toHaveLength(1);
    expect(cmp.canSave()).toBe(false); // no name yet
    await vi.advanceTimersByTimeAsync(300);
    await settle(fixture);
    const anyRule = expect.anything() as SmartRule;
    expect(stub.invoke).toHaveBeenCalledWith('preview_smart_rule', { rule: anyRule });
    expect(cmp.matchCount()).toBe(42);
  });

  it('loads an existing smart playlist name + rule when editing', async () => {
    const stored: SmartRule = {
      ...defaultRule(),
      match_all: false,
      root: { match_all: false, children: [{ field: 'genre', op: 'is', value: 'Jazz' }] },
    };
    const { fixture, cmp, ui, library } = setup(async (cmd) => {
      if (cmd === 'get_smart_playlist_rule') return stored;
      if (cmd === 'preview_smart_rule') return 3;
      return defaultInvoke(cmd);
    });
    library.playlists.set([
      { id: 7, name: 'Jazzy', kind: 'smart', parentId: null, sortOrder: 0, trackCount: 3 },
    ]);
    ui.smartEditor.set({ playlistId: 7 });
    fixture.detectChanges();
    await settle(fixture);
    expect(cmp.editingId()).toBe(7);
    expect(cmp.name()).toBe('Jazzy');
    expect(cmp.rule().match_all).toBe(false);
    expect(cmp.rows()[0]).toEqual({ field: 'genre', op: 'is', value: 'Jazz' });
  });

  it('row editing keeps values shape-consistent with field kind and operator', () => {
    const { cmp, ui } = setup();
    ui.smartEditor.set({ playlistId: null });
    cmp.setField(0, 'year');
    expect(cmp.rows()[0].op).toBe('is');
    expect(cmp.rows()[0].value).toBe(0);
    expect(cmp.widget(cmp.rows()[0])).toBe('number');
    cmp.setOp(0, 'in_range');
    expect(cmp.rows()[0].value).toEqual({ from: 0, to: 0 });
    cmp.setRange(0, 'to', '1999');
    expect(cmp.rows()[0].value).toEqual({ from: 0, to: 1999 });
    cmp.setField(0, 'last_played');
    // in_range is valid for dates too, so the operator (and range) survive.
    expect(cmp.rows()[0].op).toBe('in_range');
    cmp.setOp(0, 'in_the_last');
    expect(cmp.rows()[0].value).toEqual({ n: 30, unit: 'days' });
    cmp.setRelative(0, 'n', '0'); // clamps to 1
    cmp.setRelative(0, 'unit', 'weeks');
    expect(cmp.rows()[0].value).toEqual({ n: 1, unit: 'weeks' });
    cmp.setField(0, 'loved');
    expect(cmp.widget(cmp.rows()[0])).toBe('bool');
    expect(cmp.rows()[0].value).toBe(true);
    cmp.addRow();
    cmp.setText(1, 'Radio');
    expect(cmp.rows()).toHaveLength(2);
    expect(cmp.rows()[1].value).toBe('Radio');
    cmp.removeRow(0);
    expect(cmp.rows()).toHaveLength(1);
    cmp.removeRow(0); // never below one row
    expect(cmp.rows()).toHaveLength(1);
  });

  it('match-any and limit controls write through to the rule', () => {
    const { cmp, ui } = setup();
    ui.smartEditor.set({ playlistId: null });
    cmp.setMatchAll(false);
    expect(cmp.rule().match_all).toBe(false);
    expect(cmp.rule().root.match_all).toBe(false);
    cmp.toggleLimit(true);
    expect(cmp.rule().limit).toEqual({ value: 25, unit: 'songs', selected_by: 'random' });
    cmp.setLimitValue('-4');
    expect(cmp.rule().limit?.value).toBe(1);
    cmp.toggleLimit(false);
    expect(cmp.rule().limit).toBeNull();
  });

  it('save() creates a new playlist, refreshes the sidebar and closes', async () => {
    const { fixture, cmp, ui, stub } = setup(async (cmd) => {
      if (cmd === 'create_smart_playlist') return 99;
      if (cmd === 'list_playlists') {
        return [
          {
            id: 99,
            name: 'Mine',
            kind: 'smart',
            parent_id: null,
            sort_order: 0,
            cached_track_count: null,
          },
        ];
      }
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: null });
    fixture.detectChanges();
    cmp.name.set('  Mine ');
    expect(cmp.canSave()).toBe(true);
    await cmp.save();
    const anyRule = expect.anything() as SmartRule;
    expect(stub.invoke).toHaveBeenCalledWith('create_smart_playlist', {
      name: 'Mine',
      rule: anyRule,
    });
    expect(ui.smartEditor()).toBeNull();
  });

  it('save() on an existing playlist updates rule + name; a failure keeps the sheet open', async () => {
    let fail = true;
    const { fixture, cmp, ui, stub } = setup(async (cmd) => {
      if (cmd === 'update_smart_playlist' && fail) throw new Error('bad rule');
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 5 });
    fixture.detectChanges();
    await settle(fixture);
    cmp.name.set('Renamed');
    await cmp.save();
    expect(ui.smartEditor()).toEqual({ playlistId: 5 });
    expect(ui.lastError()).toBe('bad rule');
    fail = false;
    await cmp.save();
    expect(stub.invoke).toHaveBeenCalledWith('rename_playlist', { playlistId: 5, name: 'Renamed' });
    expect(ui.smartEditor()).toBeNull();
  });

  it('a preview failure blanks the badge instead of surfacing an error', async () => {
    const { fixture, cmp, ui } = setup(async (cmd) => {
      if (cmd === 'preview_smart_rule') throw new Error('malformed');
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: null });
    fixture.detectChanges();
    await vi.advanceTimersByTimeAsync(300);
    await settle(fixture);
    expect(cmp.matchCount()).toBeNull();
    expect(ui.lastError()).toBeNull();
  });
});
