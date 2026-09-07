import { Injectable, OnDestroy, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';

/** Live state of the bulk consolidate pass. */
export interface ConsolidateProgress {
  current: number;
  total: number;
}

/** Summary the backend emits when the consolidate pass finishes. */
export interface ConsolidateResult {
  total: number;
  moved: number;
  copied: number;
  in_place: number;
  /** Rows whose file is not on disk — nothing to consolidate. */
  missing: number;
  failed: number;
}

/** What reclaiming would free, before doing it. */
export interface ReclaimEstimate {
  files: number;
  bytes: number;
}

/** Live state of the reclaim pass. */
export interface ReclaimProgress {
  current: number;
  total: number;
}

/** Summary the backend emits when the reclaim pass finishes. */
export interface ReclaimResult {
  reclaimed: number;
  bytes_freed: number;
  skipped: number;
  failed: number;
}

@Injectable({ providedIn: 'root' })
export class PreferencesService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  readonly libraryRoot = signal<string>('');
  readonly organizeScheme = signal<string>('');
  readonly keepOrganized = signal<boolean>(true);

  /** Non-null while the consolidate pass is running. */
  readonly consolidateProgress = signal<ConsolidateProgress | null>(null);
  /** Summary of the last finished pass, or null if none ran this session. */
  readonly consolidateResult = signal<ConsolidateResult | null>(null);

  /** What a reclaim would free right now, or null before it is asked. */
  readonly reclaimEstimate = signal<ReclaimEstimate | null>(null);
  /** Non-null while the reclaim pass is running. */
  readonly reclaimProgress = signal<ReclaimProgress | null>(null);
  /** Summary of the last finished reclaim, or null if none ran. */
  readonly reclaimResult = signal<ReclaimResult | null>(null);

  private readonly unlisteners: UnlistenFn[] = [];

  constructor() {
    void this.subscribeConsolidate().catch((error: unknown) =>
      console.error('failed to subscribe to consolidate events', error),
    );
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribeConsolidate(): Promise<void> {
    this.unlisteners.push(
      await this.tauri.listen<ConsolidateProgress>('fs:consolidate-progress', (p) => {
        this.consolidateProgress.set(p);
      }),
      await this.tauri.listen<ConsolidateResult>('fs:consolidate-complete', (r) => {
        this.consolidateProgress.set(null);
        this.consolidateResult.set(r);
        // Copying in leaves originals behind, so what is reclaimable
        // has just changed.
        void this.refreshReclaimEstimate();
      }),
      await this.tauri.listen<ReclaimProgress>('fs:reclaim-progress', (p) => {
        this.reclaimProgress.set(p);
      }),
      await this.tauri.listen<ReclaimResult>('fs:reclaim-complete', (r) => {
        this.reclaimProgress.set(null);
        this.reclaimResult.set(r);
        void this.refreshReclaimEstimate();
      }),
    );
  }

  /**
   * Start the consolidate pass. Resolves as soon as it is queued: the
   * work itself reports through the two signals above.
   */
  async consolidateLibrary(): Promise<void> {
    this.consolidateResult.set(null);
    this.consolidateProgress.set({ current: 0, total: 0 });
    try {
      await this.tauri.invoke<void>('consolidate_library');
    } catch (error) {
      this.consolidateProgress.set(null);
      throw error;
    }
  }

  async refresh(): Promise<void> {
    const [root, scheme, keep] = await Promise.all([
      this.tauri.invoke<string>('get_library_root'),
      this.tauri.invoke<string>('get_organize_scheme'),
      this.tauri.invoke<boolean>('get_keep_organized'),
    ]);
    this.libraryRoot.set(root);
    this.organizeScheme.set(scheme);
    this.keepOrganized.set(keep);
  }

  /** Ask what a reclaim would free. Cheap enough to call on open. */
  async refreshReclaimEstimate(): Promise<void> {
    this.reclaimEstimate.set(await this.tauri.invoke<ReclaimEstimate>('reclaimable_originals'));
  }

  /**
   * Trash the originals of files copied into the managed library. Each
   * copy is verified byte-identical first, so this is not "delete the
   * source" — it is "the source is provably redundant".
   */
  async reclaimOriginals(): Promise<void> {
    this.reclaimResult.set(null);
    this.reclaimProgress.set({ current: 0, total: 0 });
    try {
      await this.tauri.invoke<void>('reclaim_originals');
    } catch (error) {
      this.reclaimProgress.set(null);
      throw error;
    }
  }

  async setLibraryRoot(path: string): Promise<void> {
    await this.tauri.invoke<void>('set_library_root', { path });
    this.libraryRoot.set(path);
  }

  async setOrganizeScheme(scheme: string): Promise<void> {
    await this.tauri.invoke<void>('set_organize_scheme', { scheme });
    this.organizeScheme.set(scheme);
  }

  async setKeepOrganized(keep: boolean): Promise<void> {
    await this.tauri.invoke<void>('set_keep_organized', { keep });
    this.keepOrganized.set(keep);
  }
}
