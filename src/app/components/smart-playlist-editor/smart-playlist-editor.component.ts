import {
  Component,
  OnDestroy,
  computed,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import {
  ConditionGroup,
  LeafCondition,
  LimitUnit,
  OPS_FOR_KIND,
  OP_LABELS,
  SMART_FIELDS,
  SelectionMode,
  SmartFieldKind,
  SmartOp,
  SmartRule,
  SmartValue,
  TimeUnit,
  defaultLeaf,
  defaultRule,
  defaultValue,
  fieldById,
  isGroup,
} from '../../models/smart';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';

/**
 * iTunes-style smart playlist sheet: "Match [all|any] of the following
 * rules", one row per rule (field · operator · value), optional
 * "Limit to N <unit> selected by <mode>", "Live updating", and a live
 * "N songs match" badge. Opened via `UiService.smartEditor`.
 */
@Component({
  selector: 'app-smart-playlist-editor',
  imports: [],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './smart-playlist-editor.component.html',
})
export class SmartPlaylistEditorComponent implements OnDestroy {
  protected readonly ui = inject(UiService);
  private readonly library = inject(LibraryService);

  protected readonly fields = SMART_FIELDS;
  protected readonly opLabels = OP_LABELS;
  protected readonly limitUnits: readonly LimitUnit[] = ['songs', 'minutes', 'hours', 'mb', 'gb'];
  protected readonly selectionModes: readonly { id: SelectionMode; label: string }[] = [
    { id: 'random', label: 'random' },
    { id: 'song_name', label: 'song name' },
    { id: 'album', label: 'album' },
    { id: 'artist', label: 'artist' },
    { id: 'genre', label: 'genre' },
    { id: 'most_recently_added', label: 'most recently added' },
    { id: 'most_often_played', label: 'most often played' },
    { id: 'most_recently_played', label: 'most recently played' },
    { id: 'highest_rating', label: 'highest rating' },
  ];
  protected readonly timeUnits: readonly TimeUnit[] = ['days', 'weeks', 'months'];

  protected readonly open = computed(this.#computeOpen.bind(this));
  protected readonly editingId = computed(this.#computeEditingId.bind(this));

  protected readonly name = signal('');
  protected readonly rule = signal<SmartRule>(defaultRule());
  /** Live match count; null while a preview is in flight or after an error. */
  protected readonly matchCount = signal<number | null>(null);
  protected readonly saving = signal(false);
  protected readonly loading = signal(false);

  /** Flat view of the root group's leaf rules (v1 editor is one level deep). */
  protected readonly rows = computed(this.#computeRows.bind(this));

  #computeOpen(): boolean {
    return this.ui.smartEditor() !== null;
  }

  #computeEditingId(): number | null {
    return this.ui.smartEditor()?.playlistId ?? null;
  }

  #computeRows(): LeafCondition[] {
    return this.rule().root.children.filter((c): c is LeafCondition => !isGroup(c));
  }

  private previewTimer: ReturnType<typeof setTimeout> | null = null;
  private previewSeq = 0;

  constructor() {
    // (Re)load whenever the sheet opens for a different target.
    effect(() => {
      const target = this.ui.smartEditor();
      if (target === null) return;
      void this.load(target.playlistId);
    });
    // Debounced live preview on every rule change while open.
    effect(() => {
      const rule = this.rule();
      if (!this.open()) return;
      this.schedulePreview(rule);
    });
  }

  ngOnDestroy(): void {
    if (this.previewTimer !== null) clearTimeout(this.previewTimer);
  }

  private async load(playlistId: number | null): Promise<void> {
    this.matchCount.set(null);
    if (playlistId === null) {
      this.name.set('');
      this.rule.set(defaultRule());
      return;
    }
    this.loading.set(true);
    const existing = this.library.playlists().find((p) => p.id === playlistId);
    this.name.set(existing?.name ?? '');
    const rule = await this.ui.guard(this.library.getSmartRule(playlistId));
    this.rule.set(rule ?? defaultRule());
    this.loading.set(false);
  }

  private schedulePreview(rule: SmartRule): void {
    if (this.previewTimer !== null) clearTimeout(this.previewTimer);
    const seq = ++this.previewSeq;
    this.previewTimer = setTimeout(() => {
      this.previewTimer = null;
      void this.runPreview(rule, seq);
    }, 250);
  }

  private async runPreview(rule: SmartRule, seq: number): Promise<void> {
    try {
      const n = await this.library.previewSmartRule(rule);
      if (seq === this.previewSeq) this.matchCount.set(n);
    } catch {
      // A half-typed rule (e.g. empty range) is expected to fail preview;
      // the badge just goes blank until the rule is valid.
      if (seq === this.previewSeq) this.matchCount.set(null);
    }
  }

  // ----- rule editing ----------------------------------------------------

  protected fieldKind(row: LeafCondition): SmartFieldKind {
    return fieldById(row.field).kind;
  }

  protected opsFor(row: LeafCondition): readonly SmartOp[] {
    return OPS_FOR_KIND[this.fieldKind(row)];
  }

  protected setMatchAll(all: boolean): void {
    this.rule.update((r) => ({ ...r, match_all: all, root: { ...r.root, match_all: all } }));
  }

  protected setLiveUpdating(on: boolean): void {
    this.rule.update((r) => ({ ...r, live_updating: on }));
  }

  protected addRow(): void {
    this.updateChildren((c) => [...c, defaultLeaf()]);
  }

  protected removeRow(index: number): void {
    this.updateChildren((c) => (c.length > 1 ? c.filter((_, i) => i !== index) : c));
  }

  protected setField(index: number, field: string): void {
    const kind = fieldById(field).kind;
    const ops = OPS_FOR_KIND[kind];
    this.updateRow(index, (row) => {
      const op = ops.includes(row.op) ? row.op : ops[0];
      return { field, op, value: defaultValue(kind, op) };
    });
  }

  protected setOp(index: number, op: SmartOp): void {
    this.updateRow(index, (row) => {
      const kind = fieldById(row.field).kind;
      const keep = valueShape(row.value) === valueShape(defaultValue(kind, op));
      return { ...row, op, value: keep ? row.value : defaultValue(kind, op) };
    });
  }

  protected setText(index: number, text: string): void {
    this.updateRow(index, (row) => ({ ...row, value: text }));
  }

  protected setNumber(index: number, raw: string): void {
    const n = Number(raw);
    this.updateRow(index, (row) => ({ ...row, value: Number.isFinite(n) ? n : 0 }));
  }

  protected setBool(index: number, on: boolean): void {
    this.updateRow(index, (row) => ({ ...row, value: on }));
  }

  protected setRange(index: number, part: 'from' | 'to', raw: string): void {
    const n = Number(raw);
    this.updateRow(index, (row) => {
      const cur = isRange(row.value) ? row.value : { from: 0, to: 0 };
      return { ...row, value: { ...cur, [part]: Number.isFinite(n) ? n : 0 } };
    });
  }

  protected setRelative(index: number, part: 'n' | 'unit', raw: string): void {
    this.updateRow(index, (row) => {
      const cur = isRelative(row.value) ? row.value : { n: 30, unit: 'days' as TimeUnit };
      if (part === 'unit') return { ...row, value: { ...cur, unit: raw as TimeUnit } };
      const n = Number(raw);
      return { ...row, value: { ...cur, n: Number.isFinite(n) && n > 0 ? Math.floor(n) : 1 } };
    });
  }

  // ----- limit -----------------------------------------------------------

  protected toggleLimit(on: boolean): void {
    this.rule.update((r) => ({
      ...r,
      limit: on ? (r.limit ?? { value: 25, unit: 'songs', selected_by: 'random' }) : null,
    }));
  }

  protected setLimitValue(raw: string): void {
    const n = Math.floor(Number(raw));
    this.rule.update((r) =>
      r.limit ? { ...r, limit: { ...r.limit, value: Number.isFinite(n) && n > 0 ? n : 1 } } : r,
    );
  }

  protected setLimitUnit(unit: LimitUnit): void {
    this.rule.update((r) => (r.limit ? { ...r, limit: { ...r.limit, unit } } : r));
  }

  protected setSelectedBy(mode: SelectionMode): void {
    this.rule.update((r) => (r.limit ? { ...r, limit: { ...r.limit, selected_by: mode } } : r));
  }

  // ----- value accessors for the template ---------------------------------

  protected textValue(row: LeafCondition): string {
    return typeof row.value === 'string' ? row.value : '';
  }

  protected numberValue(row: LeafCondition): number {
    return typeof row.value === 'number' ? row.value : 0;
  }

  protected boolValue(row: LeafCondition): boolean {
    return row.value === true;
  }

  protected rangeValue(row: LeafCondition): { from: number; to: number } {
    return isRange(row.value) ? row.value : { from: 0, to: 0 };
  }

  protected relativeValue(row: LeafCondition): { n: number; unit: TimeUnit } {
    return isRelative(row.value) ? row.value : { n: 30, unit: 'days' };
  }

  /** Which input widget a row needs, from its operator + field kind. */
  protected widget(row: LeafCondition): 'text' | 'number' | 'bool' | 'range' | 'relative' {
    if (row.op === 'in_range') return 'range';
    if (row.op === 'in_the_last' || row.op === 'not_in_the_last') return 'relative';
    const kind = this.fieldKind(row);
    if (kind === 'bool') return 'bool';
    if (kind === 'text') return 'text';
    return 'number';
  }

  // ----- actions -----------------------------------------------------------

  protected canSave(): boolean {
    return this.name().trim().length > 0 && !this.saving() && !this.loading();
  }

  protected async save(): Promise<void> {
    if (!this.canSave()) return;
    this.saving.set(true);
    const id = this.editingId();
    const name = this.name().trim();
    const write: Promise<unknown> =
      id === null
        ? this.library.createSmartPlaylist(name, this.rule())
        : this.library.updateSmartPlaylist(id, name, this.rule());
    const ok = await this.ui.guard(write);
    this.saving.set(false);
    if (ok !== null) this.close();
  }

  protected close(): void {
    this.ui.smartEditor.set(null);
  }

  protected onNameInput(event: Event): void {
    this.name.set((event.target as HTMLInputElement).value);
  }

  protected inputValue(event: Event): string {
    return (event.target as HTMLInputElement | HTMLSelectElement).value;
  }

  protected inputChecked(event: Event): boolean {
    return (event.target as HTMLInputElement).checked;
  }

  private updateChildren(
    fn: (children: (LeafCondition | ConditionGroup)[]) => (LeafCondition | ConditionGroup)[],
  ): void {
    this.rule.update((r) => ({ ...r, root: { ...r.root, children: fn(r.root.children) } }));
  }

  private updateRow(index: number, fn: (row: LeafCondition) => LeafCondition): void {
    this.updateChildren((children) =>
      children.map((c, i) => (i === index && !isGroup(c) ? fn(c) : c)),
    );
  }
}

function isRange(v: SmartValue): v is { from: number; to: number } {
  return typeof v === 'object' && v !== null && 'from' in v;
}

function isRelative(v: SmartValue): v is { n: number; unit: TimeUnit } {
  return typeof v === 'object' && v !== null && 'n' in v;
}

function valueShape(v: SmartValue): string {
  if (isRange(v)) return 'range';
  if (isRelative(v)) return 'relative';
  return typeof v;
}
