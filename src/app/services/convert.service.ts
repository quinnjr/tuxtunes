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

/** Tally of two batches from one run. `format` becomes "a+b" if they differ. */
export function mergeComplete(a: ConvertComplete, b: ConvertComplete): ConvertComplete {
  const formats = new Set([...a.format.split('+'), b.format]);
  return {
    total: a.total + b.total,
    converted: a.converted + b.converted,
    failed: a.failed + b.failed,
    addedToLibrary: a.addedToLibrary + b.addedToLibrary,
    cancelled: a.cancelled || b.cancelled,
    format: [...formats].join('+'),
  };
}

@Injectable({ providedIn: 'root' })
export class ConvertService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  /** null until `refresh()` has answered; the UI shows a neutral state. */
  readonly available = signal<boolean | null>(null);
  readonly prefs = signal<ConvertPrefs>(DEFAULT_CONVERT_PREFS);
  readonly progress = signal<ConvertProgress | null>(null);
  readonly lastComplete = signal<ConvertComplete | null>(null);
  /** Per-file failures from the current run; cleared when a run starts. */
  readonly failures = signal<ConvertFailure[]>([]);
  /**
   * Batches the backend has accepted and not yet reported complete. The
   * worker emits exactly one complete event per batch, cancelled or not,
   * so this counts down to idle on its own. Counting rather than a flag
   * covers the gap before a batch's first progress event and a second
   * batch queued behind the first.
   */
  readonly pending = signal(0);

  readonly running = computed(this.#computeRunning.bind(this));

  #computeRunning(): boolean {
    return this.pending() > 0;
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
      await this.tauri.listen<ConvertProgress>('fs:convert-progress', (raw) =>
        this.progress.set(raw),
      ),
      await this.tauri.listen<{
        total: number;
        converted: number;
        failed: number;
        added_to_library: number;
        cancelled: boolean;
        format: string;
      }>('fs:convert-complete', (raw) => {
        const done: ConvertComplete = {
          total: raw.total,
          converted: raw.converted,
          failed: raw.failed,
          addedToLibrary: raw.added_to_library,
          cancelled: raw.cancelled,
          format: raw.format,
        };
        // Batches queued back to back are one run to the user: the
        // summary is their sum, not whichever finished last.
        this.lastComplete.update((prev) => (prev ? mergeComplete(prev, done) : done));
        this.pending.update((n) => Math.max(0, n - 1));
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

  /**
   * Queue a batch using the saved prefs. Resolves once it is queued.
   * State changes only after the backend has accepted the batch: a
   * rejected queue (bad prefs blob, worker gone) must not leave a
   * phantom "running" with a Cancel button and no event to clear it.
   */
  async convert(trackIds: number[], format: ConvertFormat): Promise<void> {
    await this.tauri.invoke<void>('convert_tracks', {
      args: { track_ids: trackIds, format },
    });
    if (this.pending() === 0) {
      this.progress.set(null);
      this.lastComplete.set(null);
      this.failures.set([]);
    }
    this.pending.update((n) => n + 1);
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
