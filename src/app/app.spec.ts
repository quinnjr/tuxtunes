import { signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { App, mediaKeyAction } from './app';
import { LibraryService } from './services/library.service';
import { PlaybackService } from './services/playback.service';
import { UiService } from './services/ui.service';
import { WindowService } from './services/window.service';
import { appProviders, tauriStub } from './test-helpers';

describe('App', () => {
  it('refreshes library stats on init', () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({
      imports: [App],
      providers: appProviders(stub),
    });
    const fixture = TestBed.createComponent(App);
    const library = TestBed.inject(LibraryService);
    const spy = vi.spyOn(library, 'refreshStats').mockResolvedValue();
    fixture.detectChanges();
    expect(spy).toHaveBeenCalled();
  });

  describe('media keys', () => {
    function setup() {
      const stub = tauriStub();
      TestBed.configureTestingModule({
        imports: [App],
        providers: appProviders(stub),
      });
      const fixture = TestBed.createComponent(App);
      fixture.detectChanges();
      const playback = TestBed.inject(PlaybackService);
      const spies = {
        togglePlay: vi.spyOn(playback, 'togglePlay').mockResolvedValue(),
        resume: vi.spyOn(playback, 'resume').mockResolvedValue(),
        pause: vi.spyOn(playback, 'pause').mockResolvedValue(),
        stop: vi.spyOn(playback, 'stop').mockResolvedValue(),
        next: vi.spyOn(playback, 'next').mockResolvedValue(null),
        previous: vi.spyOn(playback, 'previous').mockResolvedValue(),
      };
      const cmp = fixture.componentInstance as unknown as {
        onMediaKey(e: KeyboardEvent): void;
      };
      return { cmp, spies };
    }

    it.each([
      ['MediaPlayPause', 'togglePlay'],
      ['MediaPlay', 'resume'],
      ['MediaPause', 'pause'],
      ['MediaStop', 'stop'],
      ['MediaTrackNext', 'next'],
      ['MediaTrackPrevious', 'previous'],
    ] as const)('%s drives playback.%s', (key, method) => {
      const { cmp, spies } = setup();
      const event = new KeyboardEvent('keydown', { key, cancelable: true });
      cmp.onMediaKey(event);
      expect(spies[method]).toHaveBeenCalledOnce();
      expect(event.defaultPrevented).toBe(true);
      for (const [name, spy] of Object.entries(spies)) {
        if (name !== method) expect(spy).not.toHaveBeenCalled();
      }
    });

    it('ignores ordinary keys', () => {
      const { cmp, spies } = setup();
      const event = new KeyboardEvent('keydown', { key: ' ', cancelable: true });
      cmp.onMediaKey(event);
      for (const spy of Object.values(spies)) expect(spy).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(false);
    });

    it('mediaKeyAction returns null for non-media keys', () => {
      expect(mediaKeyAction('Enter')).toBeNull();
      expect(mediaKeyAction('MediaTrackNext')).toBe('next');
    });
  });

  describe('F11', () => {
    function setupWithWindow(customControls: boolean) {
      const stub = tauriStub();
      const win = {
        customControls: signal(customControls),
        nativeTrafficLights: signal(false),
        hairline: signal(false),
        maximized: signal(false),
        fullscreen: signal(false),
        toggleFullscreen: vi.fn(async () => undefined),
      };
      TestBed.configureTestingModule({
        imports: [App],
        providers: [...appProviders(stub), { provide: WindowService, useValue: win }],
      });
      const fixture = TestBed.createComponent(App);
      fixture.detectChanges();
      const cmp = fixture.componentInstance as unknown as {
        onFullscreenKey(e: Event): void;
      };
      return { cmp, win };
    }

    it('toggles fullscreen where the app draws its own window controls', () => {
      const { cmp, win } = setupWithWindow(true);
      const event = new KeyboardEvent('keydown', { key: 'F11', cancelable: true });
      cmp.onFullscreenKey(event);
      expect(win.toggleFullscreen).toHaveBeenCalledOnce();
      expect(event.defaultPrevented).toBe(true);
    });

    it('is left to the OS where the window chrome is native', () => {
      const { cmp, win } = setupWithWindow(false);
      const event = new KeyboardEvent('keydown', { key: 'F11', cancelable: true });
      cmp.onFullscreenKey(event);
      expect(win.toggleFullscreen).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(false);
    });
  });

  it('suppresses the native context menu on ordinary elements', () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({ imports: [App], providers: appProviders(stub) });
    const fixture = TestBed.createComponent(App);
    fixture.detectChanges();
    const cmp = fixture.componentInstance as unknown as {
      onDocumentContextMenu(e: MouseEvent): void;
    };
    const preventDefault = vi.fn();
    const div = document.createElement('div');
    cmp.onDocumentContextMenu({ target: div, preventDefault } as unknown as MouseEvent);
    expect(preventDefault).toHaveBeenCalled();
  });

  it('keeps the native context menu on editable elements', () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({ imports: [App], providers: appProviders(stub) });
    const fixture = TestBed.createComponent(App);
    fixture.detectChanges();
    const cmp = fixture.componentInstance as unknown as {
      onDocumentContextMenu(e: MouseEvent): void;
    };
    const editableDiv = document.createElement('div');
    // jsdom never implements the isContentEditable getter; define the
    // property outright so the test exercises the real branch.
    Object.defineProperty(editableDiv, 'isContentEditable', { value: true });
    for (const el of [
      document.createElement('input'),
      document.createElement('textarea'),
      editableDiv,
    ]) {
      const preventDefault = vi.fn();
      cmp.onDocumentContextMenu({ target: el, preventDefault } as unknown as MouseEvent);
      expect(preventDefault, el.tagName).not.toHaveBeenCalled();
    }
  });

  it('keeps the native context menu while text is selected, for right-click copy', () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({ imports: [App], providers: appProviders(stub) });
    const fixture = TestBed.createComponent(App);
    fixture.detectChanges();
    const cmp = fixture.componentInstance as unknown as {
      onDocumentContextMenu(e: MouseEvent): void;
    };
    const getSelection = vi
      .spyOn(globalThis, 'getSelection')
      .mockReturnValue({ isCollapsed: false, toString: () => 'some text' } as unknown as Selection);
    try {
      const preventDefault = vi.fn();
      cmp.onDocumentContextMenu({
        target: document.createElement('div'),
        preventDefault,
      } as unknown as MouseEvent);
      expect(preventDefault).not.toHaveBeenCalled();
    } finally {
      getSelection.mockRestore();
    }
  });

  it('does not throw and reports the error when refreshStats rejects on init', async () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({
      imports: [App],
      providers: appProviders(stub),
    });
    const fixture = TestBed.createComponent(App);
    const library = TestBed.inject(LibraryService);
    const ui = TestBed.inject(UiService);
    vi.spyOn(library, 'refreshStats').mockRejectedValue(new Error('stats unavailable'));

    expect(() => fixture.detectChanges()).not.toThrow();
    await fixture.whenStable();

    expect(ui.lastError()).toContain('stats unavailable');
  });
});
