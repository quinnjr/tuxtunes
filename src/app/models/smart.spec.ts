import { describe, expect, it } from 'vitest';
import {
  OPS_FOR_KIND,
  SMART_FIELDS,
  SmartFieldKind,
  SmartOp,
  defaultLeaf,
  defaultRule,
  defaultValue,
  fieldById,
} from './smart';

describe('SMART_FIELDS', () => {
  it('has no duplicate ids', () => {
    const ids = SMART_FIELDS.map((f) => f.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe('OPS_FOR_KIND', () => {
  it('matches the backend operator allowlist per field kind', () => {
    const expected: Record<SmartFieldKind, readonly SmartOp[]> = {
      text: ['is', 'is_not', 'contains', 'not_contains', 'starts_with', 'ends_with'],
      int: ['is', 'is_not', 'greater', 'less', 'in_range'],
      bool: ['is'],
      date: ['in_the_last', 'not_in_the_last', 'greater', 'less', 'in_range'],
    };
    expect(OPS_FOR_KIND).toEqual(expected);
  });
});

describe('defaultValue', () => {
  it('returns a from/to range for in_range regardless of kind', () => {
    expect(defaultValue('int', 'in_range')).toEqual({ from: 0, to: 0 });
    expect(defaultValue('date', 'in_range')).toEqual({ from: 0, to: 0 });
  });

  it('returns an n/unit pair for in_the_last and not_in_the_last', () => {
    expect(defaultValue('date', 'in_the_last')).toEqual({ n: 30, unit: 'days' });
    expect(defaultValue('date', 'not_in_the_last')).toEqual({ n: 30, unit: 'days' });
  });

  it('returns true for bool', () => {
    expect(defaultValue('bool', 'is')).toBe(true);
  });

  it('returns 0 for int/date with any other op', () => {
    expect(defaultValue('int', 'is')).toBe(0);
    expect(defaultValue('date', 'greater')).toBe(0);
  });

  it('returns an empty string for text', () => {
    expect(defaultValue('text', 'contains')).toBe('');
  });
});

describe('defaultLeaf', () => {
  it('starts on artist/contains/empty string', () => {
    expect(defaultLeaf()).toEqual({ field: 'artist', op: 'contains', value: '' });
  });
});

describe('defaultRule', () => {
  it('is a match-all, live-updating rule with a single default leaf and no limit', () => {
    expect(defaultRule()).toEqual({
      match_all: true,
      live_updating: true,
      limit: null,
      root: { match_all: true, children: [defaultLeaf()] },
    });
  });
});

describe('fieldById', () => {
  it('resolves a known id', () => {
    expect(fieldById('artist')).toEqual({ id: 'artist', label: 'Artist', kind: 'text' });
  });

  it('falls back to the first field for an unknown id', () => {
    expect(fieldById('not_a_real_field')).toEqual(SMART_FIELDS[0]);
  });
});
