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
}

function setup() {
  const stub = tauriStub();
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
});
