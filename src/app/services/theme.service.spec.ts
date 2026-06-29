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
});
