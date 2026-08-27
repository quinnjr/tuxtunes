import { describe, expect, it } from 'vitest';
import { toErrorMessage } from './errors';

describe('toErrorMessage', () => {
  it('returns the message of an Error instance', () => {
    expect(toErrorMessage(new Error('nope'))).toBe('nope');
  });

  it('stringifies non-Error values', () => {
    expect(toErrorMessage('boom')).toBe('boom');
    expect(toErrorMessage(42)).toBe('42');
    expect(toErrorMessage(null)).toBe('null');
  });
});
