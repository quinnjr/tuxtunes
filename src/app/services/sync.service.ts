import { Injectable, OnDestroy, computed, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';
import { toErrorMessage } from '../utils/errors';
import {
  ConflictRules,
  PathMapping,
  SyncComplete,
  SyncFailed,
  SyncProgress,
  SyncSource,
  SyncSourceRaw,
  SyncWarning,
  mapSource,
} from '../models/sync';

interface AddSyncSourceArgs {
  args: {
    name: string;
    source_path: string;
    path_mappings: PathMapping[];
    conflict_rules: ConflictRules;
    auto_copy_files: boolean;
  };
}

@Injectable({ providedIn: 'root' })
export class SyncService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  readonly sources = signal<SyncSource[]>([]);
  readonly progress = signal<SyncProgress | null>(null);
  readonly warnings = signal<SyncWarning[]>([]);
  readonly lastComplete = signal<SyncComplete | null>(null);
  readonly lastError = signal<SyncFailed | null>(null);
  readonly logLines = signal<string[]>([]);

  /**
   * Coarse run state derived from the event signals. `runNow()` resets
   * all four signals before invoking, so a present `progress` with
   * neither `lastComplete` nor `lastError` set unambiguously means the
   * current run is still in flight (`running`); a present `lastError`
   * means it ended in failure (`error`); otherwise `idle`.
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
        source_id: number;
        phase: SyncProgress['phase'];
        current: number;
        total: number;
        message: string;
      }>('sync:progress', (raw) =>
        this.progress.set({
          sourceId: raw.source_id,
          phase: raw.phase,
          current: raw.current,
          total: raw.total,
          message: raw.message,
        }),
      ),
      await this.tauri.listen<{
        source_id: number;
        kind: SyncWarning['kind'];
        detail: string;
      }>('sync:warning', (raw) =>
        this.warnings.update((cur) => [
          ...cur.slice(-49),
          {
            sourceId: raw.source_id,
            kind: raw.kind,
            detail: raw.detail,
          },
        ]),
      ),
      await this.tauri.listen<{
        source_id: number;
        inserted_tracks: number;
        updated_tracks: number;
        deleted_tracks: number;
        inserted_playlists: number;
        updated_playlists: number;
        deleted_playlists: number;
      }>('sync:complete', (raw) =>
        this.lastComplete.set({
          sourceId: raw.source_id,
          insertedTracks: raw.inserted_tracks,
          updatedTracks: raw.updated_tracks,
          deletedTracks: raw.deleted_tracks,
          insertedPlaylists: raw.inserted_playlists,
          updatedPlaylists: raw.updated_playlists,
          deletedPlaylists: raw.deleted_playlists,
        }),
      ),
      await this.tauri.listen<{ source_id: number; error: string }>('sync:failed', (raw) =>
        this.lastError.set({ sourceId: raw.source_id, error: raw.error }),
      ),
      await this.tauri.listen<{ source_id: number; seq: number; line: string }>('sync:log', (raw) =>
        this.logLines.update((cur) => [...cur, raw.line].slice(-1000)),
      ),
    );
  }

  async refreshSources(): Promise<void> {
    const raws = await this.tauri.invoke<SyncSourceRaw[]>('list_sync_sources');
    this.sources.set(raws.map((raw) => mapSource(raw)));
  }

  async addSource(args: AddSyncSourceArgs['args']): Promise<number> {
    const id = await this.tauri.invoke<number>('add_sync_source', { args });
    await this.refreshSources();
    return id;
  }

  async runNow(sourceId: number): Promise<void> {
    this.progress.set(null);
    this.warnings.set([]);
    this.lastComplete.set(null);
    this.lastError.set(null);
    this.logLines.set([]);
    try {
      await this.tauri.invoke<void>('run_sync_now', { sourceId });
    } catch (error) {
      this.lastError.set({
        sourceId,
        error: toErrorMessage(error),
      });
    }
  }
}
