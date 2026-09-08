import { Injectable, OnDestroy, computed, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';

export type ConvertFormat = 'flac' | 'm4a';
export type M4aCodec = 'alac' | 'aac';

/**
 * Mirrors `fs::convert::ConvertPrefs`. Field names are snake_case
 * because they cross the IPC boundary as serde field names.
 */
export interface FlacPrefs {
  compression_level: number;
  sample_rate: number | null;
  bit_depth: number | null;
}

export interface M4aPrefs {
  codec: M4aCodec;
  bit_depth: number | null;
  sample_rate: number | null;
  vbr: boolean;
  vbr_quality: number;
  bitrate_kbps: number;
}

export interface ConvertPrefs {
  flac: FlacPrefs;
  m4a: M4aPrefs;
  output_dir: string | null;
  overwrite: boolean;
  add_to_library: boolean;
}

export interface ConvertProgress {
  current: number;
  total: number;
  title: string;
  /** Progress through the current file, or null when its duration is unknown. */
  percent: number | null;
}

export interface ConvertComplete {
  total: number;
  converted: number;
  failed: number;
  addedToLibrary: number;
  cancelled: boolean;
  format: string;
}

export interface ConvertFailure {
  trackId: number;
  title: string;
  error: string;
}

/**
 * Where a batch is in its life. `running` covers the gap between
 * `convert()` resolving and the worker's first progress event, which a
 * "progress but no completion yet" check would miss.
 */
export type ConvertPhase = 'idle' | 'running' | 'done';

/** Highest-quality defaults, matching the Rust `Default` impls. */
export const DEFAULT_CONVERT_PREFS: ConvertPrefs = {
  flac: { compression_level: 12, sample_rate: null, bit_depth: null },
  m4a: {
    codec: 'alac',
    bit_depth: null,
    sample_rate: null,
    vbr: false,
    vbr_quality: 2,
    bitrate_kbps: 256,
  },
  output_dir: null,
  overwrite: false,
  add_to_library: true,
};

@Injectable({ providedIn: 'root' })
export class ConvertService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  /** null until `refresh()` has answered; the UI shows a neutral state. */
  readonly available = signal<boolean | null>(null);
  readonly prefs = signal<ConvertPrefs>(DEFAULT_CONVERT_PREFS);
  readonly progress = signal<ConvertProgress | null>(null);
  readonly lastComplete = signal<ConvertComplete | null>(null);
  /** Per-file failures from the batch in flight; cleared on each start. */
  readonly failures = signal<ConvertFailure[]>([]);
  readonly phase = signal<ConvertPhase>('idle');

  readonly running = computed(this.#computeRunning.bind(this));

  #computeRunning(): boolean {
    return this.phase() === 'running';
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
      // A progress event also marks the phase: a batch queued behind
      // one that just completed starts here, not through `convert()`.
      await this.tauri.listen<ConvertProgress>('fs:convert-progress', (raw) => {
        this.progress.set(raw);
        this.phase.set('running');
      }),
      await this.tauri.listen<{
        total: number;
        converted: number;
        failed: number;
        added_to_library: number;
        cancelled: boolean;
        format: string;
      }>('fs:convert-complete', (raw) => {
        this.lastComplete.set({
          total: raw.total,
          converted: raw.converted,
          failed: raw.failed,
          addedToLibrary: raw.added_to_library,
          cancelled: raw.cancelled,
          format: raw.format,
        });
        this.phase.set('done');
      }),
      await this.tauri.listen<{ track_id: number; title: string; error: string }>(
        'fs:convert-failed',
        (raw) =>
          this.failures.update((cur) => [
            ...cur.slice(-49),
            { trackId: raw.track_id, title: raw.title, error: raw.error },
          ]),
      ),
    );
  }

  /** Load ffmpeg availability and the stored prefs together. */
  async refresh(): Promise<void> {
    const [available, prefs] = await Promise.all([
      this.tauri.invoke<boolean>('convert_available'),
      this.tauri.invoke<ConvertPrefs>('get_convert_prefs'),
    ]);
    this.available.set(available);
    this.prefs.set(prefs);
  }

  /**
   * Persist prefs. The backend clamps every knob and returns what it
   * actually stored, so adopt that rather than the optimistic draft.
   */
  async savePrefs(prefs: ConvertPrefs): Promise<void> {
    this.prefs.set(await this.tauri.invoke<ConvertPrefs>('set_convert_prefs', { prefs }));
  }

  /** Queue a batch using the saved prefs. Resolves once it is queued. */
  async convert(trackIds: number[], format: ConvertFormat): Promise<void> {
    this.progress.set(null);
    this.lastComplete.set(null);
    this.failures.set([]);
    this.phase.set('running');
    await this.tauri.invoke<void>('convert_tracks', {
      args: { track_ids: trackIds, format },
    });
  }

  /**
   * Stop the batch in flight. The worker emits its own complete event
   * with `cancelled: true`, so the running state clears from there
   * rather than being guessed at here.
   */
  async cancel(): Promise<void> {
    await this.tauri.invoke<void>('cancel_convert');
  }
}
