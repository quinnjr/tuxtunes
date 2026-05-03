import { describe, expect, it } from 'vitest';
import { UiService } from './ui.service';

describe('UiService', () => {
  it('initializes every signal to its default', () => {
    const svc = new UiService();
    expect(svc.importWizardOpen()).toBe(false);
    expect(svc.preferencesOpen()).toBe(false);
    expect(svc.libraryView()).toBe('tracks');
    expect(svc.columnBrowserOpen()).toBe(false);
    expect(svc.nowPlayingOpen()).toBe(false);
  });

  it('lets the consumer toggle each open-state signal', () => {
    const svc = new UiService();
    svc.importWizardOpen.set(true);
    svc.preferencesOpen.set(true);
    svc.columnBrowserOpen.set(true);
    svc.nowPlayingOpen.set(true);
    expect(svc.importWizardOpen()).toBe(true);
    expect(svc.preferencesOpen()).toBe(true);
    expect(svc.columnBrowserOpen()).toBe(true);
    expect(svc.nowPlayingOpen()).toBe(true);
  });

  it('accepts every LibraryView variant', () => {
    const svc = new UiService();
    for (const view of ['tracks', 'albums', 'artists', 'genres', 'settings'] as const) {
      svc.libraryView.set(view);
      expect(svc.libraryView()).toBe(view);
    }
  });
});
