import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SettingsRouteComponent } from './settings-route.component';

interface RouteInternals {
  tab: { (): string; set(v: 'playback' | 'sync' | 'maintenance' | 'about'): void };
  verifyState: { (): 'idle' | 'running' | 'done' };
  setTab(t: 'playback' | 'sync' | 'maintenance' | 'about'): void;
  formatLastSync(iso: string | null): string;
  runSync(id: number): Promise<void>;
  openImportWizard(): void;
  openLibraryPrefs(): void;
  verify(): Promise<void>;
}

function setup(invoke: (cmd: string) => Promise<unknown> = defaultInvoke) {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [SettingsRouteComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SettingsRouteComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as RouteInternals,
    library: TestBed.inject(LibraryService),
    sync: TestBed.inject(SyncService),
    ui: TestBed.inject(UiService),
    stub,
  };
}

describe('SettingsRouteComponent', () => {
  it('starts on the Playback tab', () => {
    const { cmp } = setup();
    expect(cmp.tab()).toBe('playback');
  });

  it('setTab switches the active tab', () => {
    const { cmp } = setup();
    cmp.setTab('sync');
    expect(cmp.tab()).toBe('sync');
    cmp.setTab('maintenance');
    expect(cmp.tab()).toBe('maintenance');
  });

  it('formatLastSync handles null / parseable / unparseable inputs', () => {
    const { cmp } = setup();
    expect(cmp.formatLastSync(null)).toBe('Never');
    const parsed = cmp.formatLastSync('2024-01-01T00:00:00Z');
    expect(parsed).not.toBe('2024-01-01T00:00:00Z');
    // Unparseable input round-trips so the user sees the raw string.
    expect(cmp.formatLastSync('not a date')).toBe('not a date');
  });

  it('runSync forwards to SyncService.runNow', async () => {
    const { cmp, sync } = setup();
    const spy = vi.spyOn(sync, 'runNow').mockResolvedValue();
    await cmp.runSync(7);
    expect(spy).toHaveBeenCalledWith(7);
  });

  it('openImportWizard / openLibraryPrefs set the UI signals', () => {
    const { cmp, ui } = setup();
    cmp.openImportWizard();
    expect(ui.importWizardOpen()).toBe(true);
    cmp.openLibraryPrefs();
    expect(ui.preferencesOpen()).toBe(true);
  });

  describe('verify()', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('transitions running → done and refreshes stats', async () => {
      const { cmp, library } = setup(async (cmd) => {
        if (cmd === 'verify_library') return undefined;
        return defaultInvoke(cmd);
      });
      const refresh = vi.spyOn(library, 'refreshStats').mockResolvedValue();
      const promise = cmp.verify();
      await promise;
      expect(cmp.verifyState()).toBe('running');
      vi.advanceTimersByTime(1500);
      // setTimeout's callback awaits refreshStats; let microtasks settle.
      await Promise.resolve();
      expect(refresh).toHaveBeenCalled();
      expect(cmp.verifyState()).toBe('done');
    });

    it('falls back to idle if verify_library throws', async () => {
      const { cmp } = setup(async (cmd) => {
        if (cmd === 'verify_library') throw new Error('nope');
        return defaultInvoke(cmd);
      });
      await cmp.verify();
      expect(cmp.verifyState()).toBe('idle');
    });
  });
});
