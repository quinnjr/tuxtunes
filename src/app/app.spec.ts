import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { App } from './app';
import { LibraryService } from './services/library.service';
import { UiService } from './services/ui.service';
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
