import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub, type TauriStub } from '../../test-helpers';
import { SettingsAudioComponent } from './settings-audio.component';

interface AudioInternals {
  devices: { (): unknown[] };
  selectedId: { (): string | null; set(v: string | null): void };
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

  it('select() rolls back selectedId and reports the error when set_audio_device rejects', async () => {
    let deviceCalls = 0;
    const { cmp } = setup(async (cmd) => {
      if (cmd === 'set_audio_device') {
        deviceCalls += 1;
        if (deviceCalls > 1) throw new Error('device busy');
        return undefined;
      }
      return defaultInvoke(cmd);
    });
    const ui = TestBed.inject(UiService);
    await new Promise((r) => setTimeout(r, 0));
    await cmp.select('dev-a');
    expect(cmp.selectedId()).toBe('dev-a');

    await expect(cmp.select('dev-b')).resolves.toBeUndefined();
    expect(cmp.selectedId()).toBe('dev-a');
    expect(ui.lastError()).toContain('device busy');
  });

  it('toggleExclusive() rolls back and reports the error when set_audio_device rejects', async () => {
    const { cmp } = setup(async (cmd) => {
      if (cmd === 'set_audio_device') throw new Error('locked');
      return defaultInvoke(cmd);
    });
    const ui = TestBed.inject(UiService);
    await new Promise((r) => setTimeout(r, 0));

    await expect(cmp.toggleExclusive()).resolves.toBeUndefined();
    // No selected device yet: flips with no write, no rollback, no error.
    expect(cmp.exclusive()).toBe(true);
    expect(ui.lastError()).toBeNull();

    // Select a device directly on the signal — select() itself would also
    // hit the throwing set_audio_device stub.
    cmp.selectedId.set('dev-a');
    await expect(cmp.toggleExclusive()).resolves.toBeUndefined();
    expect(cmp.exclusive()).toBe(true);
    expect(ui.lastError()).toContain('locked');
  });

  it('setReplayGain() rolls back and reports the error when set_audio_device rejects and a device is selected', async () => {
    const { cmp } = setup(async (cmd) => {
      if (cmd === 'set_audio_device') throw new Error('io error');
      return defaultInvoke(cmd);
    });
    const ui = TestBed.inject(UiService);
    await new Promise((r) => setTimeout(r, 0));
    cmp.selectedId.set('dev-a');

    await expect(cmp.setReplayGain('track')).resolves.toBeUndefined();
    expect(cmp.replayGainMode()).toBe('off');
    expect(ui.lastError()).toContain('io error');
  });

  it('refresh() failure on init does not throw and reports the error', async () => {
    const { fixture } = setup(async (cmd) => {
      if (cmd === 'list_audio_devices') throw new Error('enumeration failed');
      return defaultInvoke(cmd);
    });
    const ui = TestBed.inject(UiService);
    expect(() => fixture.detectChanges()).not.toThrow();
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
    expect(ui.lastError()).toContain('enumeration failed');
  });
});
