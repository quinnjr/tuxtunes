import { Provider, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { WindowService } from '../../services/window.service';
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
  exit(): Promise<void>;
  onEscape(): void;
  onQuitShortcut(event: Event): void;
}

function setup(extraProviders: Provider[] = []) {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [MenuBarComponent],
    providers: [...appProviders(stub), ...extraProviders],
  });
  const fixture = TestBed.createComponent(MenuBarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as MenuBarInternals,
    library: TestBed.inject(LibraryService),
    ui: TestBed.inject(UiService),
    win: TestBed.inject(WindowService),
    stub,
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

  it('addFile() delegates to LibraryService.addTracksFromPicker and closes the menu', async () => {
    const { cmp, library } = setup();
    const spy = vi.spyOn(library, 'addTracksFromPicker').mockResolvedValue(null);
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

  it('addFile() closes the menu and reports the error when addTracksFromPicker rejects', async () => {
    const { cmp, library, ui } = setup();
    vi.spyOn(library, 'addTracksFromPicker').mockRejectedValue(new Error('picker failed'));
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

  describe('as the window title bar', () => {
    it('is a deep drag region with the window controls pushed to the right edge', () => {
      const { fixture } = setup();
      const nav = (fixture.nativeElement as HTMLElement).querySelector('nav');
      expect(nav?.getAttribute('data-tauri-drag-region')).toBe('deep');
      const controls = nav?.querySelector('app-window-controls');
      expect(controls).not.toBeNull();
      expect(nav?.lastElementChild).toBe(controls);
      expect(controls?.classList.contains('ml-auto')).toBe(true);
    });

    it('drops its right padding only when it draws the caption buttons', () => {
      const customControls = signal(false);
      const { fixture } = setup([
        {
          provide: WindowService,
          useValue: {
            nativeTrafficLights: signal(false),
            customControls,
            maximized: signal(false),
            fullscreen: signal(false),
          },
        },
      ]);
      const nav = (fixture.nativeElement as HTMLElement).querySelector('nav');
      expect(nav?.classList.contains('pr-0')).toBe(false);
      customControls.set(true);
      fixture.detectChanges();
      expect(nav?.classList.contains('pr-0')).toBe(true);
    });

    it('opts the popovers and the click catcher out of dragging', () => {
      const { fixture, cmp } = setup();
      cmp.toggle('file');
      fixture.detectChanges();
      const el = fixture.nativeElement as HTMLElement;
      const optedOut = [...el.querySelectorAll('[data-tauri-drag-region="false"]')];
      expect(optedOut.some((n) => n.getAttribute('role') === 'menu')).toBe(true);
      expect(optedOut.some((n) => n.getAttribute('role') === 'presentation')).toBe(true);
    });

    it('pads past the native traffic lights only when macOS draws them', () => {
      const nativeTrafficLights = signal(false);
      const { fixture } = setup([
        {
          provide: WindowService,
          useValue: { nativeTrafficLights, customControls: signal(false) },
        },
      ]);
      const nav = (fixture.nativeElement as HTMLElement).querySelector('nav');
      expect(nav?.classList.contains('pl-20')).toBe(false);
      nativeTrafficLights.set(true);
      fixture.detectChanges();
      expect(nav?.classList.contains('pl-20')).toBe(true);
    });
  });

  describe('Exit', () => {
    /** A WindowService that reports the app is running under Tauri. */
    const inTauri = (quit = vi.fn().mockResolvedValue(undefined)) => ({
      provide: WindowService,
      useValue: {
        available: true,
        nativeTrafficLights: signal(false),
        customControls: signal(true),
        maximized: signal(false),
        fullscreen: signal(false),
        quit,
      },
    });

    /** The File menu's Exit button, or null when it is not rendered. */
    const exitButton = (host: HTMLElement): HTMLButtonElement | null =>
      [...host.querySelectorAll<HTMLButtonElement>('[role="menu"] button')].find((b) =>
        (b.textContent ?? '').includes('Exit'),
      ) ?? null;

    it('is in the File menu, after a separator, with its accelerator', () => {
      const { fixture, cmp } = setup([inTauri()]);
      cmp.toggle('file');
      fixture.detectChanges();

      const host = fixture.nativeElement as HTMLElement;
      const button = exitButton(host);
      expect(button).not.toBeNull();
      expect(button?.textContent).toContain('Ctrl+Q');
      // A role-less div in a role="menu" is announced as a nameless row.
      expect(button?.previousElementSibling?.getAttribute('role')).toBe('separator');
    });

    it('clicking it quits and closes the menu', () => {
      const quit = vi.fn().mockResolvedValue(undefined);
      const { fixture, cmp } = setup([inTauri(quit)]);
      cmp.toggle('file');
      fixture.detectChanges();

      exitButton(fixture.nativeElement as HTMLElement)?.click();

      expect(quit).toHaveBeenCalled();
      expect(cmp.openMenu()).toBeNull();
    });

    it('is not offered in a plain browser, where it could only fail', () => {
      const { fixture, cmp } = setup();
      cmp.toggle('file');
      fixture.detectChanges();
      expect(exitButton(fixture.nativeElement as HTMLElement)).toBeNull();
    });

    it('reports a refused quit instead of leaving the menu open', async () => {
      const quit = vi.fn().mockRejectedValue(new Error('no window'));
      const { cmp, ui } = setup([inTauri(quit)]);
      cmp.toggle('file');

      await cmp.exit();

      expect(cmp.openMenu()).toBeNull();
      expect(ui.lastError()).toBe('no window');
    });

    it('Ctrl+Q quits, but not while the user is typing', () => {
      const quit = vi.fn().mockResolvedValue(undefined);
      const { cmp } = setup([inTauri(quit)]);

      cmp.onQuitShortcut({
        target: document.createElement('input'),
        preventDefault: vi.fn(),
      } as unknown as Event);
      expect(quit).not.toHaveBeenCalled();

      cmp.onQuitShortcut({
        target: document.body,
        preventDefault: vi.fn(),
      } as unknown as Event);
      expect(quit).toHaveBeenCalled();
    });
  });

  it('Escape closes an open menu', () => {
    const { cmp } = setup();
    cmp.toggle('file');
    // The click catcher has no tabindex and is not an ancestor of the
    // menu, so this has to be handled on the host to work at all.
    cmp.onEscape();
    expect(cmp.openMenu()).toBeNull();
  });

  it('keeps the click catcher outside the backdrop-filtered nav', () => {
    const { fixture, cmp } = setup();
    cmp.toggle('file');
    fixture.detectChanges();

    const host = fixture.nativeElement as HTMLElement;
    const catcher = host.querySelector('[role="presentation"]');
    expect(catcher).not.toBeNull();
    // .mac-toolbar sets backdrop-filter, which would make the nav the
    // containing block for a fixed-position child: inset-0 would then
    // cover the toolbar strip instead of the viewport.
    expect(catcher?.closest('nav')).toBeNull();
  });
});
