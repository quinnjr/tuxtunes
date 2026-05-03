import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { appProviders, defaultInvoke, tauriStub, type TauriStub } from '../../test-helpers';
import { SettingsAudioComponent } from './settings-audio.component';

interface AudioInternals {
  devices: { (): unknown[] };
  selectedId: { (): string | null };
  exclusive: { (): boolean };
  replayGainMode: { (): string };
  refresh(): Promise<void>;
  select(id: string): Promise<void>;
  toggleExclusive(): Promise<void>;
  setReplayGain(mode: 'off' | 'track' | 'album'): Promise<void>;
}

function setup(invoke: (cmd: string) => Promise<unknown>): {
  fixture: ReturnType<typeof TestBed.createComponent<SettingsAudioComponent>>;
  cmp: AudioInternals;
  stub: TauriStub;
} {
  const stub = tauriStub(invoke);
  TestBed.configureTestingModule({
    imports: [SettingsAudioComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SettingsAudioComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as AudioInternals,
    stub,
  };
}

describe('SettingsAudioComponent', () => {
  it('hydrates devices + prefs on init', async () => {
    const { cmp } = setup(async (cmd) => {
      if (cmd === 'list_audio_devices')
        return [{ id: 'alsa', description: 'ALSA', supports_exclusive: true, supports_dsd: false }];
      if (cmd === 'get_audio_prefs')
        return { device_id: 'alsa', exclusive: true, replaygain_mode: 'track' };
      return undefined;
    });
    // refresh runs in ngOnInit; await one turn for it to settle.
    await Promise.resolve();
    await Promise.resolve();
    expect(cmp.devices().length).toBeGreaterThan(0);
    expect(cmp.selectedId()).toBe('alsa');
    expect(cmp.exclusive()).toBe(true);
    expect(cmp.replayGainMode()).toBe('track');
  });

  it('select() pushes the new device + current toggles to set_audio_device', async () => {
    const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
    const { cmp } = setup(async (cmd) => {
      calls.push({ cmd });
      return defaultInvoke(cmd);
    });
    // ngOnInit's refresh kicks off async; let it settle before the test
    // mutates state, otherwise refresh() races and overwrites the
    // signals we're asserting.
    await new Promise((r) => setTimeout(r, 0));
    await cmp.select('pulse');
    expect(cmp.selectedId()).toBe('pulse');
    expect(calls.some((c) => c.cmd === 'set_audio_device')).toBe(true);
  });

  it('toggleExclusive flips the state and only writes if a device is selected', async () => {
    const { cmp } = setup(defaultInvoke);
    await new Promise((r) => setTimeout(r, 0));
    await cmp.toggleExclusive();
    // No device → no write, but exclusive flipped.
    expect(cmp.exclusive()).toBe(true);
    await cmp.select('a');
    await cmp.toggleExclusive();
    expect(cmp.exclusive()).toBe(false);
  });

  it('setReplayGain updates the signal and only writes when a device is selected', async () => {
    const { cmp } = setup(defaultInvoke);
    await new Promise((r) => setTimeout(r, 0));
    await cmp.setReplayGain('album');
    expect(cmp.replayGainMode()).toBe('album');
  });
});
