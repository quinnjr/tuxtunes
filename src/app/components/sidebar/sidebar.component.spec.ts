import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { SidebarComponent } from './sidebar.component';

interface SidebarInternals {
  add(): Promise<void>;
  openImportWizard(): void;
  openPreferences(): void;
  setView(v: string): void;
  isActive(v: string): boolean;
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [SidebarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SidebarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as SidebarInternals,
    library: TestBed.inject(LibraryService),
    ui: TestBed.inject(UiService),
  };
}

describe('SidebarComponent', () => {
  it('add() delegates to LibraryService.addTrackFromPicker', async () => {
    const { cmp, library } = setup();
    const spy = vi.spyOn(library, 'addTrackFromPicker').mockResolvedValue(null);
    await cmp.add();
    expect(spy).toHaveBeenCalled();
  });

  it('openImportWizard() flips the UI signal', () => {
    const { cmp, ui } = setup();
    expect(ui.importWizardOpen()).toBe(false);
    cmp.openImportWizard();
    expect(ui.importWizardOpen()).toBe(true);
  });

  it('openPreferences() flips the UI signal', () => {
    const { cmp, ui } = setup();
    cmp.openPreferences();
    expect(ui.preferencesOpen()).toBe(true);
  });

  it('setView("genres") opens the column browser and keeps libraryView=tracks', () => {
    const { cmp, ui } = setup();
    cmp.setView('genres');
    expect(ui.libraryView()).toBe('tracks');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('setView() for other views switches libraryView without touching columnBrowser', () => {
    const { cmp, ui } = setup();
    ui.columnBrowserOpen.set(true);
    cmp.setView('albums');
    expect(ui.libraryView()).toBe('albums');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('isActive() resolves the genres pseudo-view via the column browser flag', () => {
    const { cmp, ui } = setup();
    ui.libraryView.set('tracks');
    ui.columnBrowserOpen.set(true);
    expect(cmp.isActive('genres')).toBe(true);
    expect(cmp.isActive('tracks')).toBe(false);
    ui.columnBrowserOpen.set(false);
    expect(cmp.isActive('genres')).toBe(false);
    expect(cmp.isActive('tracks')).toBe(true);
  });

  it('renders the All Songs / Artists / Albums / Genres buttons', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    for (const label of ['All Songs', 'Artists', 'Albums', 'Genres']) {
      expect(text).toContain(label);
    }
  });
});
