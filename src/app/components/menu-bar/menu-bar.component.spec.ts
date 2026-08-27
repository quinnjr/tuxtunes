import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { MenuBarComponent } from './menu-bar.component';

interface MenuBarInternals {
  openMenu(): 'file' | 'settings' | null;
  toggle(menu: 'file' | 'settings'): void;
  close(): void;
  addFile(): Promise<void>;
  addFolder(): Promise<void>;
  newSmartPlaylist(): void;
  importItunes(): void;
  openPreferences(): void;
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [MenuBarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(MenuBarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as MenuBarInternals,
    library: TestBed.inject(LibraryService),
    ui: TestBed.inject(UiService),
  };
}

describe('MenuBarComponent', () => {
  it('toggle() opens then closes the same menu', () => {
    const { cmp } = setup();
    expect(cmp.openMenu()).toBeNull();
    cmp.toggle('file');
    expect(cmp.openMenu()).toBe('file');
    cmp.toggle('file');
    expect(cmp.openMenu()).toBeNull();
  });

  it('toggle() switches directly between menus', () => {
    const { cmp } = setup();
    cmp.toggle('file');
    cmp.toggle('settings');
    expect(cmp.openMenu()).toBe('settings');
  });

  it('addFile() delegates to LibraryService.addTrackFromPicker and closes the menu', async () => {
    const { cmp, library } = setup();
    const spy = vi.spyOn(library, 'addTrackFromPicker').mockResolvedValue(null);
    cmp.toggle('file');
    await cmp.addFile();
    expect(spy).toHaveBeenCalled();
    expect(cmp.openMenu()).toBeNull();
  });

  it('addFolder() delegates to LibraryService.addFolderFromPicker and closes the menu', async () => {
    const { cmp, library } = setup();
    const spy = vi.spyOn(library, 'addFolderFromPicker').mockResolvedValue(null);
    cmp.toggle('file');
    await cmp.addFolder();
    expect(spy).toHaveBeenCalled();
    expect(cmp.openMenu()).toBeNull();
  });

  it('newSmartPlaylist() opens the editor for a new playlist and closes the menu', () => {
    const { cmp } = setup();
    const ui = TestBed.inject(UiService);
    cmp.toggle('file');
    cmp.newSmartPlaylist();
    expect(ui.smartEditor()).toEqual({ playlistId: null });
    expect(cmp.openMenu()).toBeNull();
  });

  it('importItunes() flips the import-wizard signal and closes the menu', () => {
    const { cmp, ui } = setup();
    expect(ui.importWizardOpen()).toBe(false);
    cmp.toggle('file');
    cmp.importItunes();
    expect(ui.importWizardOpen()).toBe(true);
    expect(cmp.openMenu()).toBeNull();
  });

  it('openPreferences() flips the preferences signal and closes the menu', () => {
    const { cmp, ui } = setup();
    cmp.toggle('settings');
    cmp.openPreferences();
    expect(ui.preferencesOpen()).toBe(true);
    expect(cmp.openMenu()).toBeNull();
  });

  it('renders the File and Settings menu triggers', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('File');
    expect(text).toContain('Settings');
  });

  it('addFile() closes the menu and reports the error when addTrackFromPicker rejects', async () => {
    const { cmp, library, ui } = setup();
    vi.spyOn(library, 'addTrackFromPicker').mockRejectedValue(new Error('picker failed'));
    cmp.toggle('file');

    await expect(cmp.addFile()).resolves.toBeUndefined();

    expect(cmp.openMenu()).toBeNull();
    expect(ui.lastError()).toContain('picker failed');
  });

  it('addFolder() closes the menu and reports the error when addFolderFromPicker rejects', async () => {
    const { cmp, library, ui } = setup();
    vi.spyOn(library, 'addFolderFromPicker').mockRejectedValue(new Error('folder locked'));
    cmp.toggle('file');

    await expect(cmp.addFolder()).resolves.toBeUndefined();

    expect(cmp.openMenu()).toBeNull();
    expect(ui.lastError()).toContain('folder locked');
  });
});
