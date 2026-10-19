import { readFileSync } from 'node:fs';

// The Angular unit-test builder does not hand specs an importable copy of
// src/styles.css (`?raw`/`?inline` come back empty and styles are not injected
// into jsdom), and cwd is the workspace root under `ng test`. So read the file
// directly. jsdom cannot compute `user-select`, so these pin the stylesheet
// contract itself rather than a computed style.
const css = readFileSync('src/styles.css', 'utf8');

const ruleBody = (selector: RegExp): string => selector.exec(css)?.[1] ?? '';

/** Matches a declaration without letting `-webkit-user-select` satisfy it. */
const declares = (body: string, property: string, value: string): boolean =>
  new RegExp(String.raw`(?<![-\w])${property}:\s*${value}`).test(body);

describe('styles.css text selection', () => {
  it('suppresses selection on .mac-select-none chrome', () => {
    const body = ruleBody(/\.mac-select-none\s*\{([^}]*)\}/);
    expect(declares(body, 'user-select', 'none')).toBe(true);
    expect(declares(body, '-webkit-user-select', 'none')).toBe(true);
  });

  it('opts editable fields back into selection inside .mac-select-none', () => {
    const body = ruleBody(/\.mac-select-none input[^{]*\{([^}]*)\}/);
    expect(declares(body, 'user-select', 'text')).toBe(true);
    expect(declares(body, '-webkit-user-select', 'text')).toBe(true);
  });

  it('names every editable control in the opt-in selector list', () => {
    const selectors = /\.mac-select-none input[^{]*/.exec(css)?.[0] ?? '';
    expect(selectors).toContain('.mac-select-none textarea');
    expect(selectors).toContain('.mac-select-none [contenteditable]:read-write');
  });
});
