import { Injectable, OnDestroy, computed, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import {
  Device,
  DeviceComplete,
  DeviceFailed,
  DeviceProgress,
  DeviceRaw,
  DeviceSettings,
  DeviceWarning,
  SelectionEntry,
  SyncPlanSummary,
  SyncPlanSummaryRaw,
  mapDevice,
  mapPlanSummary,
} from '../models/device';
import { TauriService } from './tauri.service';
import { toErrorMessage } from '../utils/errors';

/** Cap on retained warnings, matching SyncService. */
const MAX_WARNINGS = 50;
/** Cap on retained log lines, matching SyncService. */
const MAX_LOG_LINES = 1000;

@Injectable({ providedIn: 'root' })
export class DeviceService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  readonly devices = signal<Device[]>([]);
  readonly progress = signal<DeviceProgress | null>(null);
  readonly warnings = signal<DeviceWarning[]>([]);
  readonly lastComplete = signal<DeviceComplete | null>(null);
  readonly lastError = signal<DeviceFailed | null>(null);
  readonly logLines = signal<string[]>([]);
  readonly lastPlan = signal<SyncPlanSummary | null>(null);

  /**
   * Coarse run state derived from the event signals. `runNow()` clears
   * all four before invoking, so a present `progress` with neither
   * terminal signal set unambiguously means a run is in flight.
   */
  readonly runState = computed<'idle' | 'running' | 'error'>(this.#computeRunState.bind(this));

  #computeRunState(): 'idle' | 'running' | 'error' {
    if (this.progress() && !this.lastComplete() && !this.lastError()) return 'running';
    if (this.lastError()) return 'error';
    return 'idle';
  }

  private readonly unlisteners: UnlistenFn[] = [];

  constructor() {
    void this.subscribe();
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribe(): Promise<void> {
    this.unlisteners.push(
      await this.tauri.listen<{
        device_id: number;
        phase: DeviceProgress['phase'];
        current: number;
        total: number;
        message: string;
      }>('device:progress', (raw) =>
        this.progress.set({
          deviceId: raw.device_id,
          phase: raw.phase,
          current: raw.current,
          total: raw.total,
          message: raw.message,
        }),
      ),
      await this.tauri.listen<{
        device_id: number;
        kind: DeviceWarning['kind'];
        detail: string;
      }>('device:warning', (raw) =>
        this.warnings.update((cur) => [
          ...cur.slice(-(MAX_WARNINGS - 1)),
          { deviceId: raw.device_id, kind: raw.kind, detail: raw.detail },
        ]),
      ),
      await this.tauri.listen<{
        device_id: number;
        added: number;
        replaced: number;
        unchanged: number;
        deleted: number;
        playlists_written: number;
        skipped: number;
        bytes_written: number;
      }>('device:complete', (raw) => {
        this.lastComplete.set({
          deviceId: raw.device_id,
          added: raw.added,
          replaced: raw.replaced,
          unchanged: raw.unchanged,
          deleted: raw.deleted,
          playlistsWritten: raw.playlists_written,
          skipped: raw.skipped,
          bytesWritten: raw.bytes_written,
        });
        // A finished sync updates last_sync_at on the row.
        void this.refresh();
      }),
      await this.tauri.listen<{ device_id: number; error: string }>('device:failed', (raw) =>
        this.lastError.set({ deviceId: raw.device_id, error: raw.error }),
      ),
      await this.tauri.listen<{ device_id: number; seq: number; line: string }>(
        'device:log',
        (raw) => this.logLines.update((cur) => [...cur, raw.line].slice(-MAX_LOG_LINES)),
      ),
      await this.tauri.listen<{ device_id: number; name: string }>(
        'device:attached',
        () => void this.refresh(),
      ),
      await this.tauri.listen<{ device_id: number; name: string }>(
        'device:detached',
        () => void this.refresh(),
      ),
    );
  }

  /** One device by id, or undefined if it is not known. */
  byId(deviceId: number | null): Device | undefined {
    if (deviceId === null) return undefined;
    return this.devices().find((d) => d.id === deviceId);
  }

  async refresh(): Promise<void> {
    const raws = await this.tauri.invoke<DeviceRaw[]>('list_devices');
    this.devices.set(raws.map((raw) => mapDevice(raw)));
  }

  /** Re-stat known devices, then refresh the list. */
  async rescan(): Promise<void> {
    const raws = await this.tauri.invoke<DeviceRaw[]>('refresh_devices');
    this.devices.set(raws.map((raw) => mapDevice(raw)));
  }

  /**
   * Open the native folder picker and register the chosen mount as a
   * device. Resolves to `null` if the dialog was dismissed.
   */
  async pickAndAddDevice(): Promise<number | null> {
    const id = await this.tauri.invoke<number | null>('pick_and_add_device');
    await this.refresh();
    return id;
  }

  async addFilesystemDevice(args: {
    name: string;
    mountPath: string;
    rootPath?: string;
  }): Promise<number> {
    const id = await this.tauri.invoke<number>('add_filesystem_device', {
      args: {
        name: args.name,
        mount_path: args.mountPath,
        root_path: args.rootPath ?? null,
      },
    });
    await this.refresh();
    return id;
  }

  async updateSelection(deviceId: number, selection: SelectionEntry[]): Promise<void> {
    await this.tauri.invoke<void>('update_device_selection', { deviceId, selection });
    await this.refresh();
  }

  async updateSettings(deviceId: number, settings: DeviceSettings): Promise<void> {
    await this.tauri.invoke<void>('update_device_settings', { deviceId, settings });
    await this.refresh();
  }

  async forget(deviceId: number): Promise<void> {
    await this.tauri.invoke<void>('forget_device', { deviceId });
    await this.refresh();
  }

  /** Dry run: what a sync would do, without touching the device. */
  async preview(deviceId: number): Promise<SyncPlanSummary> {
    const raw = await this.tauri.invoke<SyncPlanSummaryRaw>('preview_device_sync', { deviceId });
    const summary = mapPlanSummary(raw);
    this.lastPlan.set(summary);
    return summary;
  }

  async runNow(deviceId: number): Promise<void> {
    this.progress.set(null);
    this.warnings.set([]);
    this.lastComplete.set(null);
    this.lastError.set(null);
    this.logLines.set([]);
    try {
      await this.tauri.invoke<void>('run_device_sync', { deviceId });
    } catch (error) {
      this.lastError.set({ deviceId, error: toErrorMessage(error) });
    }
  }

  async cancel(deviceId: number): Promise<void> {
    await this.tauri.invoke<void>('cancel_device_sync', { deviceId });
  }
}
