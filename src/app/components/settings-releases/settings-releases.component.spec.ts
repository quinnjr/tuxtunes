import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { ReleasesService } from '../../services/releases.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SettingsReleasesComponent } from './settings-releases.component';

interface Internals {
  setWindow(days: number): void;
  draft(): { window_days: number };
}

async function setup(started: boolean, fetched: number | null = started ? 90 : null) {
  const stub = tauriStub(async (cmd, args) => {
    if (cmd === 'set_release_prefs') return args?.['prefs'];
    if (cmd === 'get_new_releases') {
      return {
        checked_at: null,
        running: false,
        started,
        window_days: 90,
        fetched_window_days: fetched,
        releases: [],
        last_error: null,
        qobuz_error: null,
      };
    }
    return defaultInvoke(cmd);
  });
  TestBed.configureTestingModule({
    imports: [SettingsReleasesComponent],
    providers: [...appProviders(stub), ReleasesService],
  });
  const releases = TestBed.inject(ReleasesService);
  await releases.refresh();
  const fixture = TestBed.createComponent(SettingsReleasesComponent);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  const settle = async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  };
  return {
    fixture,
    stub,
    settle,
    el: fixture.nativeElement as HTMLElement,
    cmp: fixture.componentInstance as unknown as Internals,
  };
}

describe('SettingsReleasesComponent', () => {
  it('saves a new window and re-reads the feed, leaving any check to the backend', async () => {
    const { cmp, stub, settle } = await setup(true);
    stub.invoke.mockClear();
    cmp.setWindow(180);
    await settle();
    expect(stub.invoke).toHaveBeenCalledWith('set_release_prefs', {
      prefs: { window_days: 180, qobuz_app_id: null },
    });
    expect(stub.invoke).toHaveBeenCalledWith('get_new_releases');
    expect(stub.invoke).not.toHaveBeenCalledWith('check_new_releases');
  });

  it('saves a trimmed app ID on change, and a cleared one as null', async () => {
    const { el, stub, settle } = await setup(false);
    const input = el.querySelector<HTMLInputElement>('[data-testid="qobuz-app-id"]')!;
    input.value = '  12345 ';
    input.dispatchEvent(new Event('change'));
    await settle();
    expect(stub.invoke).toHaveBeenCalledWith('set_release_prefs', {
      prefs: { window_days: 90, qobuz_app_id: '12345' },
    });
    input.value = '';
    input.dispatchEvent(new Event('change'));
    await settle();
    expect(stub.invoke).toHaveBeenCalledWith('set_release_prefs', {
      prefs: { window_days: 90, qobuz_app_id: null },
    });
  });

  it('rolls the form back when the save fails', async () => {
    const { cmp, stub, settle } = await setup(true);
    stub.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'set_release_prefs') throw new Error('disk full');
      return defaultInvoke(cmd);
    });
    cmp.setWindow(365);
    expect(cmp.draft().window_days).toBe(365);
    await settle();
    expect(cmp.draft().window_days).toBe(90);
  });
});
