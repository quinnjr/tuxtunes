import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { PreferencesService } from '../../services/preferences.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { PreferencesPanelComponent } from './preferences-panel.component';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

import { open as dialogOpen } from '@tauri-apps/plugin-dialog';

interface PrefsInternals {
  draftRoot: { (): string; set(v: string): void };
  draftScheme: { (): string; set(v: string): void };
  draftKeep: { (): boolean; set(v: boolean): void };
  open: { (): boolean; set(v: boolean): void };
  pickRoot(): Promise<void>;
  save(): Promise<void>;
  hide(): void;
  toggleKeep(): void;
  preview(): string;
  reorganize(): Promise<void>;
  reorganizeStatus(): string | null;
  reorganizeSummary(): string | null;
  reclaim(): void;
  reclaimStatus(): string | null;
  reclaimOffer(): string | null;
  reclaimSummary(): string | null;
}

function setup(invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const stub = invoke ? tauriStub(invoke) : tauriStub();
  TestBed.configureTestingModule({
    imports: [PreferencesPanelComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(PreferencesPanelComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as PrefsInternals,
    prefs: TestBed.inject(PreferencesService),
    ui: TestBed.inject(UiService),
    stub,
  };
}

describe('PreferencesPanelComponent', () => {
  it('preview renders the default scheme when draft is empty', () => {
    const { cmp } = setup();
    cmp.draftScheme.set('');
    expect(cmp.preview()).toBe('The Beatles/Abbey Road/01-03 - Something.flac');
  });

  it('preview substitutes only known tokens for the active scheme', () => {
    const { cmp } = setup();
    cmp.draftScheme.set('{title}.{ext}');
    expect(cmp.preview()).toBe('Something.flac');
  });

  it('toggleKeep flips the boolean', () => {
    const { cmp } = setup();
    expect(cmp.draftKeep()).toBe(true);
    cmp.toggleKeep();
    expect(cmp.draftKeep()).toBe(false);
  });

  it('hide() closes the dialog', () => {
    const { cmp, ui } = setup();
    ui.preferencesOpen.set(true);
    cmp.hide();
    expect(ui.preferencesOpen()).toBe(false);
  });

  it('save() forwards each draft to PreferencesService and closes', async () => {
    const { cmp, prefs, ui } = setup();
    cmp.draftRoot.set('/r');
    cmp.draftScheme.set('{title}.{ext}');
    cmp.draftKeep.set(false);
    const r = vi.spyOn(prefs, 'setLibraryRoot').mockResolvedValue();
    const s = vi.spyOn(prefs, 'setOrganizeScheme').mockResolvedValue();
    const k = vi.spyOn(prefs, 'setKeepOrganized').mockResolvedValue();
    ui.preferencesOpen.set(true);
    await cmp.save();
    expect(r).toHaveBeenCalledWith('/r');
    expect(s).toHaveBeenCalledWith('{title}.{ext}');
    expect(k).toHaveBeenCalledWith(false);
    expect(ui.preferencesOpen()).toBe(false);
  });

  it('pickRoot stores the chosen string into draftRoot', async () => {
    const { cmp } = setup();
    (dialogOpen as ReturnType<typeof vi.fn>).mockResolvedValueOnce('/picked');
    await cmp.pickRoot();
    expect(cmp.draftRoot()).toBe('/picked');
  });

  it('pickRoot ignores a cancelled dialog (non-string return)', async () => {
    const { cmp } = setup();
    cmp.draftRoot.set('/before');
    (dialogOpen as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    await cmp.pickRoot();
    expect(cmp.draftRoot()).toBe('/before');
  });

  it('opening the dialog hydrates draft signals from PreferencesService', async () => {
    const { fixture, cmp, prefs, ui } = setup();
    vi.spyOn(prefs, 'refresh').mockImplementation(async () => {
      prefs.libraryRoot.set('/loaded');
      prefs.organizeScheme.set('{album}/{title}.{ext}');
      prefs.keepOrganized.set(false);
    });
    ui.preferencesOpen.set(true);
    fixture.detectChanges();
    // The effect schedules a microtask chain — refresh().then(write).
    // Flush a few times so jsdom's promise queue has drained.
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
    expect(cmp.draftRoot()).toBe('/loaded');
    expect(cmp.draftScheme()).toBe('{album}/{title}.{ext}');
    expect(cmp.draftKeep()).toBe(false);
  });

  it('save() reports the error and keeps the panel open when setLibraryRoot rejects', async () => {
    const { cmp, prefs, ui } = setup();
    cmp.draftRoot.set('/r');
    vi.spyOn(prefs, 'setLibraryRoot').mockRejectedValue(new Error('disk full'));
    vi.spyOn(prefs, 'setOrganizeScheme').mockResolvedValue();
    vi.spyOn(prefs, 'setKeepOrganized').mockResolvedValue();
    ui.preferencesOpen.set(true);

    await expect(cmp.save()).resolves.toBeUndefined();

    expect(ui.preferencesOpen()).toBe(true);
    expect(ui.lastError()).toContain('disk full');
  });

  it('opening the dialog leaves drafts at defaults and reports the error when refresh() rejects', async () => {
    const { fixture, cmp, prefs, ui } = setup();
    vi.spyOn(prefs, 'refresh').mockRejectedValue(new Error('unreachable'));
    ui.preferencesOpen.set(true);
    fixture.detectChanges();
    for (let i = 0; i < 5; i += 1) await Promise.resolve();

    expect(cmp.draftRoot()).toBe('');
    expect(ui.lastError()).toContain('unreachable');
  });

  describe('reorganize()', () => {
    it('saves the draft first, so the pass uses the root on screen', async () => {
      const { cmp, stub } = setup(async () => undefined);
      cmp.draftRoot.set('/music/new');
      cmp.draftScheme.set('{title}.{ext}');

      await cmp.reorganize();

      expect(stub.invoke).toHaveBeenCalledWith('set_library_root', { path: '/music/new' });
      expect(stub.invoke).toHaveBeenCalledWith('set_organize_scheme', {
        scheme: '{title}.{ext}',
      });
      expect(stub.invoke).toHaveBeenCalledWith('consolidate_library');
    });

    it('reports progress and clears it on the summary', async () => {
      const { cmp, stub } = setup(async () => undefined);
      expect(cmp.reorganizeStatus()).toBeNull();

      await cmp.reorganize();
      expect(cmp.reorganizeStatus()).toBe('Starting…');

      stub.emit('fs:consolidate-progress', { current: 7, total: 20 });
      expect(cmp.reorganizeStatus()).toBe('Reorganizing 7 of 20…');

      stub.emit('fs:consolidate-complete', {
        total: 20,
        moved: 19,
        copied: 1,
        in_place: 0,
        failed: 0,
      });
      expect(cmp.reorganizeStatus()).toBeNull();
    });

    it('does not start the pass when saving the draft fails', async () => {
      const { cmp, stub, ui } = setup(async (cmd) => {
        if (cmd === 'set_library_root') throw new Error('root is not writable');
        return undefined;
      });

      await cmp.reorganize();

      expect(stub.invoke).not.toHaveBeenCalledWith('consolidate_library');
      expect(ui.lastError()).toBe('root is not writable');
      expect(cmp.reorganizeStatus()).toBeNull();
    });
  });

  describe('reclaim()', () => {
    const estimate = { files: 23_020, bytes: 209_379_655_680 };

    it('names the files and the space before trashing anything', async () => {
      const { cmp, prefs, ui, stub } = setup(async (cmd) =>
        cmd === 'reclaimable_originals' ? estimate : undefined,
      );
      await prefs.refreshReclaimEstimate();

      cmp.reclaim();

      const req = ui.confirm();
      expect(req?.destructive).toBe(true);
      expect(req?.message).toContain('23,020');
      expect(req?.message).toContain('195 GiB');
      // Nothing happens until the user says so.
      expect(stub.invoke).not.toHaveBeenCalledWith('reclaim_originals');

      await req?.onConfirm();
      expect(stub.invoke).toHaveBeenCalledWith('reclaim_originals');
    });

    it('does nothing when there is nothing to reclaim', async () => {
      const { cmp, prefs, ui } = setup(async (cmd) =>
        cmd === 'reclaimable_originals' ? { files: 0, bytes: 0 } : undefined,
      );
      await prefs.refreshReclaimEstimate();

      cmp.reclaim();

      expect(ui.confirm()).toBeNull();
      expect(cmp.reclaimOffer()).toBe('No duplicated originals to reclaim.');
    });

    it('reports progress and then what it freed', async () => {
      const { cmp, prefs, ui, stub } = setup(async (cmd) =>
        cmd === 'reclaimable_originals' ? estimate : undefined,
      );
      await prefs.refreshReclaimEstimate();
      cmp.reclaim();
      await ui.confirm()?.onConfirm();

      expect(cmp.reclaimStatus()).toBe('Starting…');
      stub.emit('fs:reclaim-progress', { current: 500, total: 23_020 });
      expect(cmp.reclaimStatus()).toBe('Reclaiming 500 of 23020…');

      stub.emit('fs:reclaim-complete', {
        reclaimed: 23_000,
        bytes_freed: 209_379_655_680,
        skipped: 20,
        failed: 0,
      });
      expect(cmp.reclaimStatus()).toBeNull();
      expect(cmp.reclaimSummary()).toBe('23,000 trashed (195 GiB), 20 left alone');
    });
  });

  describe('reorganizeSummary()', () => {
    it('reports rows whose file is not on disk apart from failures', async () => {
      const { cmp, stub } = setup(async () => undefined);
      await Promise.resolve(); // listener registration settles
      stub.emit('fs:consolidate-complete', {
        total: 24_618,
        moved: 1,
        copied: 23_020,
        in_place: 16,
        missing: 1581,
        failed: 0,
      });
      expect(cmp.reorganizeSummary()).toBe(
        '1 moved, 23020 copied, 16 already in place, 1581 files not found',
      );
    });

    it('still names real failures', async () => {
      const { cmp, stub } = setup(async () => undefined);
      await Promise.resolve();
      stub.emit('fs:consolidate-complete', {
        total: 3,
        moved: 1,
        copied: 1,
        in_place: 0,
        missing: 0,
        failed: 1,
      });
      expect(cmp.reorganizeSummary()).toBe('1 moved, 1 copied, 0 already in place, 1 failed');
    });
  });
});
