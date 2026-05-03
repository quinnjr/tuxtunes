import { Component, OnInit, inject, signal } from '@angular/core';
import { TauriService } from '../../services/tauri.service';

export interface AudioDevice {
  id: string;
  description: string;
  supports_exclusive: boolean;
  supports_dsd: boolean;
}

export type ReplayGainMode = 'track' | 'album' | 'off';

interface AudioPrefsSnapshot {
  device_id: string | null;
  exclusive: boolean;
  replaygain_mode: ReplayGainMode;
}

@Component({
  selector: 'app-settings-audio',
  imports: [],
  templateUrl: './settings-audio.component.html',
})
export class SettingsAudioComponent implements OnInit {
  private readonly tauri = inject(TauriService);

  protected readonly devices = signal<AudioDevice[]>([]);
  protected readonly selectedId = signal<string | null>(null);
  protected readonly exclusive = signal<boolean>(false);
  protected readonly replayGainMode = signal<ReplayGainMode>('off');

  protected readonly replayGainOptions: readonly { id: ReplayGainMode; label: string }[] = [
    { id: 'off', label: 'Off' },
    { id: 'track', label: 'Track gain' },
    { id: 'album', label: 'Album gain' },
  ] as const;

  ngOnInit(): void {
    void this.refresh();
  }

  /**
   * Load both the device list and the persisted prefs in parallel.
   * The two are independent — devices come from libmpv's enumeration,
   * prefs come from the SQLite store — so concurrent fetch keeps the
   * settings tab snappy.
   */
  protected async refresh(): Promise<void> {
    const [list, prefs] = await Promise.all([
      this.tauri.invoke<AudioDevice[]>('list_audio_devices'),
      this.tauri.invoke<AudioPrefsSnapshot>('get_audio_prefs'),
    ]);
    this.devices.set(list);
    this.selectedId.set(prefs.device_id);
    this.exclusive.set(prefs.exclusive);
    this.replayGainMode.set(prefs.replaygain_mode);
  }

  protected async select(id: string): Promise<void> {
    this.selectedId.set(id);
    await this.apply();
  }

  protected async toggleExclusive(): Promise<void> {
    this.exclusive.update((v) => !v);
    if (this.selectedId()) await this.apply();
  }

  protected async setReplayGain(mode: ReplayGainMode): Promise<void> {
    this.replayGainMode.set(mode);
    if (this.selectedId()) await this.apply();
  }

  /**
   * Push the current draft to the backend. The backend persists the
   * three keys atomically — selected_device_id, exclusive, replaygain
   * — so a partial update never produces an inconsistent state.
   */
  private async apply(): Promise<void> {
    const id = this.selectedId();
    if (!id) return;
    await this.tauri.invoke<void>('set_audio_device', {
      args: {
        device_id: id,
        exclusive: this.exclusive(),
        replaygain_mode: this.replayGainMode(),
      },
    });
  }
}
