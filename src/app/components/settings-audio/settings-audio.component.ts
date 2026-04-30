import { Component, OnInit, inject, signal } from '@angular/core';
import { TauriService } from '../../services/tauri.service';

export interface AudioDevice {
  id: string;
  description: string;
  supports_exclusive: boolean;
  supports_dsd: boolean;
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

  ngOnInit(): void {
    void this.refresh();
  }

  protected async refresh(): Promise<void> {
    const list = await this.tauri.invoke<AudioDevice[]>('list_audio_devices');
    this.devices.set(list);
  }

  protected async select(id: string): Promise<void> {
    this.selectedId.set(id);
    await this.tauri.invoke<void>('set_audio_device', {
      args: { device_id: id, exclusive: this.exclusive() },
    });
  }

  protected async toggleExclusive(): Promise<void> {
    this.exclusive.update((v) => !v);
    const id = this.selectedId();
    if (id) {
      await this.tauri.invoke<void>('set_audio_device', {
        args: { device_id: id, exclusive: this.exclusive() },
      });
    }
  }
}
