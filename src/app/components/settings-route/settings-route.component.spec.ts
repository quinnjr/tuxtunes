import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { PreferencesService } from '../../services/preferences.service';
import { SyncService } from '../../services/sync.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SettingsRouteComponent } from './settings-route.component';

interface RouteInternals {
  tab: { (): string; set(v: 'playback' | 'sync' | 'maintenance' | 'about'): void };
  verifyState: { (): 'idle' | 'running' | 'done' | 'error' };
  verifyError: { (): string | null };
  setTab(t: 'playback' | 'sync' | 'maintenance' | 'about'): void;
  formatLastSync(iso: string | null): string;
  runSync(id: number): Promise<void>;
  openImportWizard(): void;
  openLibraryPrefs(): void;
  verify(): Promise<void>;
  consolidate(): Promise<void>;
  consolidateStatus(): string | null;
  consolidateError(): string | null;
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
    prefs: TestBed.inject(PreferencesService),
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

    it('sets error state and message when verify_library rejects', async () => {
      const { cmp } = setup(async (cmd) => {
        if (cmd === 'verify_library') throw new Error('nope');
        return defaultInvoke(cmd);
      });
      await cmp.verify();
      expect(cmp.verifyState()).toBe('error');
      expect(cmp.verifyError()).toBe('nope');
    });

    it('sets error state and message on fs:verify-failed event', async () => {
      const { cmp, stub } = setup(async (cmd) => {
        if (cmd === 'verify_library') return undefined;
        return defaultInvoke(cmd);
      });
      const promise = cmp.verify();
      await promise;
      expect(cmp.verifyState()).toBe('running');
      stub.emit('fs:verify-failed', { message: 'checksum mismatch' });
      expect(cmp.verifyState()).toBe('error');
      expect(cmp.verifyError()).toBe('checksum mismatch');
    });

    it('does not clobber an error state set by fs:verify-failed once the settle timer fires', async () => {
      const { cmp, library, stub } = setup(async (cmd) => {
        if (cmd === 'verify_library') return undefined;
        return defaultInvoke(cmd);
      });
      const refresh = vi.spyOn(library, 'refreshStats').mockResolvedValue();
      const promise = cmp.verify();
      await promise;
      expect(cmp.verifyState()).toBe('running');
      stub.emit('fs:verify-failed', { message: 'checksum mismatch' });
      expect(cmp.verifyState()).toBe('error');
      vi.advanceTimersByTime(1500);
      await Promise.resolve();
      expect(cmp.verifyState()).toBe('error');
      expect(cmp.verifyError()).toBe('checksum mismatch');
      expect(refresh).not.toHaveBeenCalled();
    });
  });

  describe('consolidate()', () => {
    it('reports progress from the backend and clears it on the summary', async () => {
      const { cmp, stub } = setup(async (cmd) => {
        if (cmd === 'consolidate_library') return undefined;
        return defaultInvoke(cmd);
      });
      expect(cmp.consolidateStatus()).toBeNull();

      await cmp.consolidate();
      expect(cmp.consolidateStatus()).toBe('Starting…');

      stub.emit('fs:consolidate-progress', { current: 40, total: 120 });
      expect(cmp.consolidateStatus()).toBe('Reorganizing 40 of 120…');

      stub.emit('fs:consolidate-complete', {
        total: 120,
        moved: 100,
        copied: 19,
        in_place: 1,
        failed: 0,
      });
      expect(cmp.consolidateStatus()).toBeNull();
    });

    it('reloads the track list once the pass finishes, since paths moved', async () => {
      const { cmp, library, stub } = setup(async (cmd) => {
        if (cmd === 'consolidate_library') return undefined;
        return defaultInvoke(cmd);
      });
      const refresh = vi.spyOn(library, 'refreshTracks').mockResolvedValue();
      await cmp.consolidate();
      stub.emit('fs:consolidate-complete', {
        total: 1,
        moved: 1,
        copied: 0,
        in_place: 0,
        failed: 0,
      });
      expect(refresh).toHaveBeenCalled();
    });

    it('surfaces a rejected command instead of leaving the button disabled', async () => {
      const { cmp } = setup(async (cmd) => {
        if (cmd === 'consolidate_library') throw new Error('ingest worker has exited');
        return defaultInvoke(cmd);
      });
      await cmp.consolidate();
      expect(cmp.consolidateError()).toBe('ingest worker has exited');
      expect(cmp.consolidateStatus()).toBeNull();
    });
  });
});
