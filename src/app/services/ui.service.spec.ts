import { afterEach, describe, expect, it, vi } from 'vitest';
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

  describe('error reporting', () => {
    afterEach(() => {
      vi.useRealTimers();
    });

    it('reportError() extracts a message from an Error and clearError() nulls it', () => {
      const svc = new UiService();
      svc.reportError(new Error('x'));
      expect(svc.lastError()).toBe('x');
      svc.clearError();
      expect(svc.lastError()).toBeNull();
    });

    it('reportError() accepts a plain string message', () => {
      const svc = new UiService();
      svc.reportError('plain');
      expect(svc.lastError()).toBe('plain');
    });

    it('auto-clears the error after ERROR_VISIBLE_MS, and a second report resets the timer', () => {
      vi.useFakeTimers();
      const svc = new UiService();
      svc.reportError('first');
      vi.advanceTimersByTime(5000);
      svc.reportError('second');
      vi.advanceTimersByTime(5000);
      expect(svc.lastError()).toBe('second');
      vi.advanceTimersByTime(1000);
      expect(svc.lastError()).toBeNull();
    });

    it('guard() resolves the value and leaves lastError unset on success', async () => {
      const svc = new UiService();
      await expect(svc.guard(Promise.resolve(3))).resolves.toBe(3);
      expect(svc.lastError()).toBeNull();
    });

    it('guard() resolves null and reports the error on rejection', async () => {
      const svc = new UiService();
      await expect(svc.guard(Promise.reject(new Error('bad')))).resolves.toBeNull();
      expect(svc.lastError()).toBe('bad');
    });
  });
});
