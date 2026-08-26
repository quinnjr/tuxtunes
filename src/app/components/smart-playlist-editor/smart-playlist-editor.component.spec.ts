import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ConditionGroup, LeafCondition, SmartRule, defaultRule } from '../../models/smart';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { EditorRow, SmartPlaylistEditorComponent } from './smart-playlist-editor.component';

interface EditorInternals {
  name: { (): string; set(v: string): void };
  rule: { (): SmartRule; set(v: SmartRule): void };
  rows(): EditorRow[];
  matchCount(): number | null;
  editingId(): number | null;
  canSave(): boolean;
  loadFailed(): boolean;
  retryLoad(): void;
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

/** Pull the leaf rows out of `rows()`, in display order. */
function leaves(rows: EditorRow[]): LeafCondition[] {
  return rows.flatMap((r) => ('leaf' in r ? [r.leaf] : []));
}

function groups(rows: EditorRow[]): ConditionGroup[] {
  return rows.flatMap((r) => ('group' in r ? [r.group] : []));
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
    expect(leaves(cmp.rows())).toHaveLength(1);
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
    expect(leaves(cmp.rows())[0]).toEqual({ field: 'genre', op: 'is', value: 'Jazz' });
  });

  it('row editing keeps values shape-consistent with field kind and operator', () => {
    const { cmp, ui } = setup();
    ui.smartEditor.set({ playlistId: null });
    cmp.setField(0, 'year');
    expect(leaves(cmp.rows())[0].op).toBe('is');
    expect(leaves(cmp.rows())[0].value).toBe(0);
    expect(cmp.widget(leaves(cmp.rows())[0])).toBe('number');
    cmp.setOp(0, 'in_range');
    expect(leaves(cmp.rows())[0].value).toEqual({ from: 0, to: 0 });
    cmp.setRange(0, 'to', '1999');
    expect(leaves(cmp.rows())[0].value).toEqual({ from: 0, to: 1999 });
    cmp.setField(0, 'last_played');
    // in_range is valid for dates too, so the operator (and range) survive.
    expect(leaves(cmp.rows())[0].op).toBe('in_range');
    cmp.setOp(0, 'in_the_last');
    expect(leaves(cmp.rows())[0].value).toEqual({ n: 30, unit: 'days' });
    cmp.setRelative(0, 'n', '0'); // clamps to 1
    cmp.setRelative(0, 'unit', 'weeks');
    expect(leaves(cmp.rows())[0].value).toEqual({ n: 1, unit: 'weeks' });
    cmp.setField(0, 'loved');
    expect(cmp.widget(leaves(cmp.rows())[0])).toBe('bool');
    expect(leaves(cmp.rows())[0].value).toBe(true);
    cmp.addRow();
    cmp.setText(1, 'Radio');
    expect(leaves(cmp.rows())).toHaveLength(2);
    expect(leaves(cmp.rows())[1].value).toBe('Radio');
    cmp.removeRow(0);
    expect(leaves(cmp.rows())).toHaveLength(1);
    cmp.removeRow(0); // never below one row
    expect(leaves(cmp.rows())).toHaveLength(1);
  });

  it('loading the imported iTunes shape (root -> single wrapper group) yields one editable leaf row', async () => {
    const stored: SmartRule = {
      ...defaultRule(),
      match_all: true,
      root: {
        match_all: true,
        children: [
          {
            match_all: true,
            children: [{ field: 'artist', op: 'contains', value: '10 Years' }],
          },
        ],
      },
    };
    const { fixture, cmp, ui, stub } = setup(async (cmd) => {
      if (cmd === 'get_smart_playlist_rule') return stored;
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 11 });
    fixture.detectChanges();
    await settle(fixture);
    expect(leaves(cmp.rows())).toEqual([{ field: 'artist', op: 'contains', value: '10 Years' }]);
    expect(groups(cmp.rows())).toHaveLength(0);
    cmp.name.set('Recent 10YY');
    await cmp.save();
    expect(stub.invoke).toHaveBeenCalledWith('update_smart_playlist', {
      playlistId: 11,
      rule: expect.objectContaining({
        root: {
          match_all: true,
          children: [{ field: 'artist', op: 'contains', value: '10 Years' }],
        },
      }) as SmartRule,
    });
  });

  it('a mixed root [leaf, group, leaf] edits/removes the right child', async () => {
    const leafX: LeafCondition = { field: 'genre', op: 'is', value: 'Rock' };
    const group: ConditionGroup = {
      match_all: false,
      children: [{ field: 'title', op: 'contains', value: 'live' }],
    };
    const leafY: LeafCondition = { field: 'album', op: 'is', value: 'Old' };
    const stored: SmartRule = {
      ...defaultRule(),
      root: { match_all: true, children: [leafX, group, leafY] },
    };
    const { fixture, cmp, ui } = setup(async (cmd) => {
      if (cmd === 'get_smart_playlist_rule') return stored;
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 12 });
    fixture.detectChanges();
    await settle(fixture);
    // rows: [leafX @0, group @1, leafY @2]
    expect(cmp.rows()).toHaveLength(3);
    cmp.setText(2, 'New Album'); // typing into leafY's row (real index 2)
    expect(leaves(cmp.rows())).toEqual([leafX, { ...leafY, value: 'New Album' }]);
    cmp.removeRow(1); // remove only the group
    expect(cmp.rows()).toHaveLength(2);
    expect(groups(cmp.rows())).toHaveLength(0);
    expect(leaves(cmp.rows())).toEqual([leafX, { ...leafY, value: 'New Album' }]);
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

  it('a rejected rule load reports an error, blocks save and offers a working retry', async () => {
    let fail = true;
    const { fixture, cmp, ui, stub } = setup(async (cmd) => {
      if (cmd === 'get_smart_playlist_rule') {
        if (fail) throw new Error('disk error');
        return defaultRule();
      }
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 20 });
    fixture.detectChanges();
    await settle(fixture);
    expect(cmp.loadFailed()).toBe(true);
    expect(ui.lastError()).toBe('disk error');
    cmp.name.set('Anything');
    expect(cmp.canSave()).toBe(false);
    await cmp.save();
    const calledCommands = stub.invoke.mock.calls.map((c) => c[0] as string);
    expect(calledCommands).not.toContain('update_smart_playlist');

    fail = false;
    cmp.retryLoad();
    await settle(fixture);
    expect(cmp.loadFailed()).toBe(false);
    cmp.name.set('Anything'); // retry reloads name from the library, same as any load()
    expect(cmp.canSave()).toBe(true);
  });

  it('a non-smart / unknown playlist id (null rule, no rejection) still falls back to a default rule', async () => {
    const { fixture, cmp, ui } = setup(async (cmd) => {
      if (cmd === 'get_smart_playlist_rule') return null;
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 21 });
    fixture.detectChanges();
    await settle(fixture);
    expect(cmp.loadFailed()).toBe(false);
    expect(cmp.rule()).toEqual(defaultRule());
  });

  it('a background playlists() refresh while the sheet is open does not discard unsaved edits', async () => {
    const { fixture, cmp, ui, library } = setup();
    ui.smartEditor.set({ playlistId: null });
    fixture.detectChanges();
    await settle(fixture);
    cmp.name.set('My Edits');
    library.playlists.set([
      { id: 1, name: 'Something Else', kind: 'smart', parentId: null, sortOrder: 0, trackCount: 0 },
    ]);
    fixture.detectChanges();
    await settle(fixture);
    expect(cmp.name()).toBe('My Edits');
  });

  it('a stale getSmartRule from a previously opened playlist is ignored once a new one opens', async () => {
    const slow: { resolve: ((rule: SmartRule) => void) | undefined } = { resolve: undefined };
    const slowRule: SmartRule = {
      ...defaultRule(),
      root: { match_all: true, children: [{ field: 'genre', op: 'is', value: 'Slow42' }] },
    };
    const fastRule: SmartRule = {
      ...defaultRule(),
      root: { match_all: true, children: [{ field: 'genre', op: 'is', value: 'Fast99' }] },
    };
    const { fixture, cmp, ui } = setup(async (cmd, args) => {
      if (cmd === 'get_smart_playlist_rule') {
        if ((args as { playlistId: number }).playlistId === 42) {
          return new Promise<SmartRule>((resolve) => {
            slow.resolve = resolve;
          });
        }
        return fastRule;
      }
      return defaultInvoke(cmd);
    });
    ui.smartEditor.set({ playlistId: 42 });
    fixture.detectChanges();
    await settle(fixture);
    ui.smartEditor.set({ playlistId: 99 });
    fixture.detectChanges();
    await settle(fixture);
    expect(leaves(cmp.rows())).toEqual([{ field: 'genre', op: 'is', value: 'Fast99' }]);
    slow.resolve?.(slowRule);
    await settle(fixture);
    // The late result for 42 must not clobber 99's already-loaded rule.
    expect(leaves(cmp.rows())).toEqual([{ field: 'genre', op: 'is', value: 'Fast99' }]);
  });
});
