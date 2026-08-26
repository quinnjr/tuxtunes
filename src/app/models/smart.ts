/**
 * Mirror of `src-tauri/src/db/smart.rs`. Serialised as-is to the
 * `create_smart_playlist` / `update_smart_playlist` / `preview_smart_rule`
 * commands (serde: snake_case enums, untagged `Value`/`Condition`).
 */

export type SmartFieldKind = 'text' | 'int' | 'bool' | 'date';

export interface SmartField {
  id: string;
  label: string;
  kind: SmartFieldKind;
}

/** Every field the backend's allowlist accepts, in iTunes' menu order. */
export const SMART_FIELDS: readonly SmartField[] = [
  { id: 'title', label: 'Title', kind: 'text' },
  { id: 'artist', label: 'Artist', kind: 'text' },
  { id: 'album_artist', label: 'Album Artist', kind: 'text' },
  { id: 'album', label: 'Album', kind: 'text' },
  { id: 'composer', label: 'Composer', kind: 'text' },
  { id: 'genre', label: 'Genre', kind: 'text' },
  { id: 'kind', label: 'Kind', kind: 'text' },
  { id: 'comment', label: 'Comment', kind: 'text' },
  { id: 'year', label: 'Year', kind: 'int' },
  { id: 'track_number', label: 'Track Number', kind: 'int' },
  { id: 'disc_number', label: 'Disc Number', kind: 'int' },
  { id: 'bpm', label: 'BPM', kind: 'int' },
  { id: 'duration_ms', label: 'Time (ms)', kind: 'int' },
  { id: 'size_bytes', label: 'Size (bytes)', kind: 'int' },
  { id: 'bit_rate', label: 'Bit Rate', kind: 'int' },
  { id: 'sample_rate', label: 'Sample Rate', kind: 'int' },
  { id: 'rating', label: 'Rating', kind: 'int' },
  { id: 'play_count', label: 'Plays', kind: 'int' },
  { id: 'skip_count', label: 'Skips', kind: 'int' },
  { id: 'loved', label: 'Loved', kind: 'bool' },
  { id: 'date_added', label: 'Date Added', kind: 'date' },
  { id: 'last_played', label: 'Last Played', kind: 'date' },
  { id: 'last_skipped', label: 'Last Skipped', kind: 'date' },
] as const;

export type SmartOp =
  | 'is'
  | 'is_not'
  | 'contains'
  | 'not_contains'
  | 'starts_with'
  | 'ends_with'
  | 'greater'
  | 'less'
  | 'in_range'
  | 'in_the_last'
  | 'not_in_the_last';

export const OP_LABELS: Record<SmartOp, string> = {
  is: 'is',
  is_not: 'is not',
  contains: 'contains',
  not_contains: 'does not contain',
  starts_with: 'starts with',
  ends_with: 'ends with',
  greater: 'is greater than',
  less: 'is less than',
  in_range: 'is in the range',
  in_the_last: 'is in the last',
  not_in_the_last: 'is not in the last',
};

/** Operators the backend accepts per field kind (see smart.rs `compile_leaf`). */
export const OPS_FOR_KIND: Record<SmartFieldKind, readonly SmartOp[]> = {
  text: ['is', 'is_not', 'contains', 'not_contains', 'starts_with', 'ends_with'],
  int: ['is', 'is_not', 'greater', 'less', 'in_range'],
  bool: ['is'],
  date: ['in_the_last', 'not_in_the_last', 'greater', 'less', 'in_range'],
};

export type TimeUnit = 'days' | 'weeks' | 'months';

export type SmartValue =
  string | number | boolean | { from: number; to: number } | { n: number; unit: TimeUnit };

export interface LeafCondition {
  field: string;
  op: SmartOp;
  value: SmartValue;
}

export interface ConditionGroup {
  match_all: boolean;
  children: (LeafCondition | ConditionGroup)[];
}

export type LimitUnit = 'songs' | 'minutes' | 'hours' | 'mb' | 'gb';
export type SelectionMode =
  | 'random'
  | 'song_name'
  | 'album'
  | 'artist'
  | 'genre'
  | 'most_recently_added'
  | 'most_often_played'
  | 'most_recently_played'
  | 'highest_rating';

export interface SmartLimit {
  value: number;
  unit: LimitUnit;
  selected_by: SelectionMode | null;
}

export interface SmartRule {
  match_all: boolean;
  live_updating: boolean;
  limit: SmartLimit | null;
  root: ConditionGroup;
}

export function isGroup(c: LeafCondition | ConditionGroup): c is ConditionGroup {
  return 'children' in c;
}

export function fieldById(id: string): SmartField {
  return SMART_FIELDS.find((f) => f.id === id) ?? SMART_FIELDS[0];
}

/** A sensible starting value for a field kind + operator. */
export function defaultValue(kind: SmartFieldKind, op: SmartOp): SmartValue {
  if (op === 'in_range') return { from: 0, to: 0 };
  if (op === 'in_the_last' || op === 'not_in_the_last') return { n: 30, unit: 'days' };
  if (kind === 'bool') return true;
  if (kind === 'int' || kind === 'date') return 0;
  return '';
}

export function defaultLeaf(): LeafCondition {
  return { field: 'artist', op: 'contains', value: '' };
}

export function defaultRule(): SmartRule {
  return {
    match_all: true,
    live_updating: true,
    limit: null,
    root: { match_all: true, children: [defaultLeaf()] },
  };
}
