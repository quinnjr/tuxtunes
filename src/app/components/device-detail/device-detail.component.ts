import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { Device, SelectionEntry, formatBytes } from '../../models/device';
import { DeviceService } from '../../services/device.service';
import { LibraryService, Playlist } from '../../services/library.service';
import { UiService } from '../../services/ui.service';

@Component({
  selector: 'app-device-detail',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './device-detail.component.html',
})
export class DeviceDetailComponent {
  protected readonly ui = inject(UiService);
  protected readonly devices = inject(DeviceService);
  protected readonly library = inject(LibraryService);

  /** Whether the raw sync log is expanded. */
  protected readonly logOpen = signal(false);

  protected readonly device = computed<Device | undefined>(this.#computeDevice.bind(this));
  /** Playlists and smart playlists, flat; folders cannot be synced. */
  protected readonly syncablePlaylists = computed<Playlist[]>(this.#computeSyncable.bind(this));
  protected readonly running = computed(this.#computeRunning.bind(this));
  /** Progress percentage of the current phase, floored. */
  protected readonly percent = computed(this.#computePercent.bind(this));
  protected readonly usedPercent = computed(this.#computeUsedPercent.bind(this));

  #computeDevice(): Device | undefined {
    return this.devices.byId(this.ui.activeDeviceId());
  }

  #computeSyncable(): Playlist[] {
    return this.library.playlists().filter((p) => p.kind !== 'folder');
  }

  #computeRunning(): boolean {
    return (
      this.devices.runState() === 'running' &&
      this.devices.progress()?.deviceId === this.device()?.id
    );
  }

  #computePercent(): number {
    const p = this.devices.progress();
    if (p === null || p.total === 0) return 0;
    return Math.min(100, Math.floor((p.current / p.total) * 100));
  }

  #computeUsedPercent(): number | null {
    const plan = this.devices.lastPlan();
    const total = plan?.totalBytes ?? null;
    const free = plan?.freeBytes ?? null;
    // A transport that cannot report free space leaves both null, and
    // there is no capacity bar to draw.
    if (total === null || free === null || total === 0) return null;
    return Math.round(((total - free) / total) * 100);
  }

  protected isSelected(playlist: Playlist): boolean {
    const device = this.device();
    if (!device) return false;
    const kind = playlist.kind === 'smart' ? 'smart' : 'playlist';
    return device.selection.some((e) => e.kind === kind && 'id' in e && e.id === playlist.id);
  }

  /**
   * Toggling writes the whole selection back, since the backend stores
   * it as one JSON column. Reading from the current row each time
   * means a concurrent change is not silently clobbered by a stale
   * snapshot held in the template.
   */
  protected toggle(playlist: Playlist): void {
    const device = this.device();
    if (!device) return;
    const kind = playlist.kind === 'smart' ? 'smart' : 'playlist';
    const without = device.selection.filter(
      (e) => !(e.kind === kind && 'id' in e && e.id === playlist.id),
    );
    const next: SelectionEntry[] =
      without.length === device.selection.length
        ? [...device.selection, { kind, id: playlist.id }]
        : without;
    void this.ui.guard(this.devices.updateSelection(device.id, next));
  }

  protected selectedCount(): number {
    return this.device()?.selection.length ?? 0;
  }

  protected preview(): void {
    const device = this.device();
    if (!device) return;
    void this.ui.guard(this.devices.preview(device.id));
  }

  protected sync(): void {
    const device = this.device();
    if (!device) return;
    void this.ui.guard(this.devices.runNow(device.id));
  }

  protected cancel(): void {
    const device = this.device();
    if (!device) return;
    void this.ui.guard(this.devices.cancel(device.id));
  }

  protected toggleLog(): void {
    this.logOpen.update((v) => !v);
  }

  protected setMirrorDeletes(device: Device, value: boolean): void {
    void this.ui.guard(
      this.devices.updateSettings(device.id, {
        name: device.name,
        root_path: device.rootPath,
        layout_template: device.layoutTemplate,
        auto_sync: device.autoSync,
        mirror_deletes: value,
        write_playlist_objects: device.writePlaylistObjects,
      }),
    );
  }

  protected onMirrorDeletesChange(device: Device, event: Event): void {
    this.setMirrorDeletes(device, (event.target as HTMLInputElement).checked);
  }

  protected readonly bytes = formatBytes;
}
