import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { ConvertPrefs, DEFAULT_CONVERT_PREFS } from '../../services/convert.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SettingsConvertComponent } from './settings-convert.component';

interface ConvertInternals {
  draft: { (): ConvertPrefs };
  cancel(): Promise<void>;
  patch(change: Partial<ConvertPrefs>): Promise<void>;
  patchFlac(change: Partial<ConvertPrefs['flac']>): void;
  patchM4a(change: Partial<ConvertPrefs['m4a']>): void;
  toNullableNumber(raw: string): number | null;
  resetDefaults(): void;
}

function setup(invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [SettingsConvertComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SettingsConvertComponent);
  fixture.detectChanges();
  return { fixture, cmp: fixture.componentInstance as unknown as ConvertInternals, stub };
}

const settle = async (): Promise<void> => {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
};

describe('SettingsConvertComponent', () => {
  it('hydrates the draft from the stored prefs on init', async () => {
    const stored: ConvertPrefs = {
      ...DEFAULT_CONVERT_PREFS,
      flac: { compression_level: 5, sample_rate: 48_000, bit_depth: 24 },
    };
    const { cmp } = setup(async (cmd) =>
      cmd === 'get_convert_prefs' ? stored : defaultInvoke(cmd),
    );
    await settle();
    expect(cmp.draft().flac.compression_level).toBe(5);
    expect(cmp.draft().flac.sample_rate).toBe(48_000);
  });

  it('persists a nested FLAC edit without dropping the M4A settings', async () => {
    const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
    const { cmp } = setup(async (cmd, args) => {
      calls.push({ cmd, args });
      return defaultInvoke(cmd);
    });
    await settle();

    cmp.patchFlac({ compression_level: 8 });
    await settle();

    const saved = calls.find((c) => c.cmd === 'set_convert_prefs');
    const prefs = saved?.args?.['prefs'] as ConvertPrefs;
    expect(prefs.flac.compression_level).toBe(8);
    expect(prefs.m4a).toEqual(DEFAULT_CONVERT_PREFS.m4a);
  });

  it('shows the clamped values the backend stored, not the draft that was sent', async () => {
    const { cmp } = setup(async (cmd) => {
      if (cmd === 'set_convert_prefs') {
        return {
          ...DEFAULT_CONVERT_PREFS,
          m4a: { ...DEFAULT_CONVERT_PREFS.m4a, bitrate_kbps: 512 },
        };
      }
      return defaultInvoke(cmd);
    });
    await settle();

    cmp.patchM4a({ bitrate_kbps: 4000 });
    await settle();

    expect(cmp.draft().m4a.bitrate_kbps).toBe(512);
  });

  it('the add-to-library toggle is on by default and persists when switched off', async () => {
    let lastSaved: unknown;
    const { cmp } = setup(async (cmd, args) => {
      if (cmd === 'set_convert_prefs') lastSaved = args?.['prefs'];
      return defaultInvoke(cmd);
    });
    await settle();
    expect(cmp.draft().add_to_library).toBe(true);

    cmp.patch({ add_to_library: false });
    await settle();

    expect((lastSaved as ConvertPrefs).add_to_library).toBe(false);
  });

  it('cancel() reaches the backend', async () => {
    const calls: string[] = [];
    const { cmp } = setup(async (cmd) => {
      calls.push(cmd);
      return defaultInvoke(cmd);
    });
    await settle();
    await cmp.cancel();
    expect(calls).toContain('cancel_convert');
  });

  it('maps the empty select option to "same as source"', () => {
    const { cmp } = setup(defaultInvoke);
    expect(cmp.toNullableNumber('')).toBeNull();
    expect(cmp.toNullableNumber('96000')).toBe(96_000);
  });

  it('resetDefaults() persists the highest-quality preset', async () => {
    let lastSaved: unknown;
    const { cmp } = setup(async (cmd, args) => {
      if (cmd === 'set_convert_prefs') lastSaved = args?.['prefs'];
      return defaultInvoke(cmd);
    });
    await settle();

    cmp.patchFlac({ compression_level: 0 });
    await settle();
    cmp.resetDefaults();
    await settle();

    expect(lastSaved).toEqual(DEFAULT_CONVERT_PREFS);
  });
});
