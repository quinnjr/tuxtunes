import { TestBed } from '@angular/core/testing';
import { beforeEach, describe, expect, it } from 'vitest';
import { ThemeService } from './theme.service';

function make(): ThemeService {
  localStorage.clear();
  TestBed.configureTestingModule({});
  return TestBed.inject(ThemeService);
}

describe('ThemeService', () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset['theme'];
  });

  it('defaults to system when nothing is stored', () => {
    const theme = make();
    expect(theme.mode()).toBe('system');
  });

  it('resolves explicit light/dark verbatim', () => {
    const theme = make();
    theme.set('light');
    expect(theme.resolved()).toBe('light');
    theme.set('dark');
    expect(theme.resolved()).toBe('dark');
  });

  it('resolves system to a concrete light/dark value', () => {
    const theme = make();
    theme.set('system');
    expect(['light', 'dark']).toContain(theme.resolved());
  });

  it('tracks OS prefers-color-scheme live when mode is system', () => {
    let handler: ((event: MediaQueryListEvent) => void) | undefined;
    const originalMatchMedia = globalThis.matchMedia;
    globalThis.matchMedia = ((query: string) =>
      ({
        matches: false,
        media: query,
        addEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) => {
          handler = listener;
        },
        removeEventListener: () => {},
      }) as unknown as MediaQueryList) as typeof globalThis.matchMedia;

    try {
      const theme = make();
      theme.set('system');
      expect(handler).toBeDefined();

      handler?.({ matches: true } as MediaQueryListEvent);
      expect(theme.resolved()).toBe('dark');

      handler?.({ matches: false } as MediaQueryListEvent);
      expect(theme.resolved()).toBe('light');
    } finally {
      globalThis.matchMedia = originalMatchMedia;
    }
  });
});
