import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { PreferencesService } from './preferences.service';
import { TauriService } from './tauri.service';

type InvokeMock = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/**
 * Build the PreferencesService against a stub TauriService. Using
 * `runInInjectionContext` avoids the full TestBed bootstrap while still
 * letting Angular's `inject()` resolve the dep.
 */
function build(invoke: InvokeMock): {
  svc: PreferencesService;
  invoke: ReturnType<typeof vi.fn>;
} {
  const invokeSpy = vi.fn(invoke as never);
  const stubTauri = { invoke: invokeSpy } as unknown as TauriService;
  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: PreferencesService, useClass: PreferencesService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(PreferencesService));
  return { svc, invoke: invokeSpy };
}

describe('PreferencesService', () => {
  it('initializes signals to their defaults', () => {
    const { svc } = build(async () => {});
    expect(svc.libraryRoot()).toBe('');
    expect(svc.organizeScheme()).toBe('');
    expect(svc.keepOrganized()).toBe(true);
  });

  it('refresh() pulls all three keys in parallel and writes them to signals', async () => {
    const responses: Record<string, unknown> = {
      get_library_root: '/music',
      get_organize_scheme: '{title}.{ext}',
      get_keep_organized: false,
    };
    const { svc, invoke } = build(async (cmd) => responses[cmd]);
    await svc.refresh();
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(svc.libraryRoot()).toBe('/music');
    expect(svc.organizeScheme()).toBe('{title}.{ext}');
    expect(svc.keepOrganized()).toBe(false);
  });

  it('setLibraryRoot() forwards the path and updates the signal', async () => {
    const { svc, invoke } = build(async () => {});
    await svc.setLibraryRoot('/new/path');
    expect(invoke).toHaveBeenCalledWith('set_library_root', { path: '/new/path' });
    expect(svc.libraryRoot()).toBe('/new/path');
  });

  it('setOrganizeScheme() forwards the scheme and updates the signal', async () => {
    const { svc, invoke } = build(async () => {});
    await svc.setOrganizeScheme('{album}/{title}.{ext}');
    expect(invoke).toHaveBeenCalledWith('set_organize_scheme', {
      scheme: '{album}/{title}.{ext}',
    });
    expect(svc.organizeScheme()).toBe('{album}/{title}.{ext}');
  });

  it('setKeepOrganized() forwards the flag and updates the signal', async () => {
    const { svc, invoke } = build(async () => {});
    await svc.setKeepOrganized(false);
    expect(invoke).toHaveBeenCalledWith('set_keep_organized', { keep: false });
    expect(svc.keepOrganized()).toBe(false);
  });
});
