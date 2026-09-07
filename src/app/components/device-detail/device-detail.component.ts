import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import {
  Device,
  DeviceComplete,
  DeviceFailed,
  DeviceProgress,
  DeviceWarning,
  SelectionEntry,
} from '../../models/device';
import { formatByteSize } from '../../utils/format';
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

  /**
   * Selection as the user has left it, ahead of the round-trip.
   *
   * `device().selection` only updates after `update_device_selection`
   * and the refresh that follows it both resolve, so two quick clicks
   * would each build from the same stale snapshot and the first would
   * be lost. Keyed by device id so it cannot leak across devices.
   */
  readonly #pending = signal<{ deviceId: number; selection: SelectionEntry[] } | null>(null);

  /** The selection to render and to build the next edit from. */
  #currentSelection(device: Device): SelectionEntry[] {
    const pending = this.#pending();
    return pending?.deviceId === device.id ? pending.selection : device.selection;
  }

  protected readonly device = computed<Device | undefined>(this.#computeDevice.bind(this));
  /** Playlists and smart playlists, flat; folders cannot be synced. */
  protected readonly syncablePlaylists = computed<Playlist[]>(this.#computeSyncable.bind(this));
  protected readonly running = computed(this.#computeRunning.bind(this));
  /** Progress percentage of the current phase, floored. */
  protected readonly percent = computed(this.#computePercent.bind(this));
  protected readonly usedPercent = computed(this.#computeUsedPercent.bind(this));

  // The service keeps one set of run-state signals, so everything the
  // panel shows is filtered to the device actually on screen. Without
  // this, opening device B right after syncing device A shows A's
  // progress, warnings and summary as if they were B's.
  protected readonly progress = computed(this.#computeProgress.bind(this));
  protected readonly warnings = computed(this.#computeWarnings.bind(this));
  protected readonly lastComplete = computed(this.#computeComplete.bind(this));
  protected readonly lastError = computed(this.#computeError.bind(this));

  #computeDevice(): Device | undefined {
    return this.devices.byId(this.ui.activeDeviceId());
  }

  #computeProgress(): DeviceProgress | null {
    const p = this.devices.progress();
    return p !== null && p.deviceId === this.device()?.id ? p : null;
  }

  #computeWarnings(): DeviceWarning[] {
    const id = this.device()?.id;
    return this.devices.warnings().filter((w) => w.deviceId === id);
  }

  #computeComplete(): DeviceComplete | null {
    const c = this.devices.lastComplete();
    return c !== null && c.deviceId === this.device()?.id ? c : null;
  }

  #computeError(): DeviceFailed | null {
    const e = this.devices.lastError();
    return e !== null && e.deviceId === this.device()?.id ? e : null;
  }

  #computeSyncable(): Playlist[] {
    return this.library.playlists().filter((p) => p.kind !== 'folder');
  }

  #computeRunning(): boolean {
    return this.devices.runState() === 'running' && this.progress() !== null;
  }

  #computePercent(): number {
    const p = this.progress();
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
    return this.#currentSelection(device).some(
      (e) => e.kind === kind && 'id' in e && e.id === playlist.id,
    );
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
    const current = this.#currentSelection(device);
    const without = current.filter((e) => !(e.kind === kind && 'id' in e && e.id === playlist.id));
    const next: SelectionEntry[] =
      without.length === current.length ? [...current, { kind, id: playlist.id }] : without;

    // Record the intent before the round-trip so a second click builds
    // on it rather than on the stale server copy.
    this.#pending.set({ deviceId: device.id, selection: next });
    void this.ui.guard(
      this.devices.updateSelection(device.id, next).finally(() => {
        // Drop the override only if nothing newer replaced it.
        if (this.#pending()?.selection === next) this.#pending.set(null);
      }),
    );
  }

  protected selectedCount(): number {
    const device = this.device();
    return device ? this.#currentSelection(device).length : 0;
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

  /** The app already has one byte formatter; a second one that
   * disagreed (1000-based "GB" vs 1024-based "GiB") would show two
   * different sizes for the same number in one window. */
  protected readonly bytes = formatByteSize;
}
