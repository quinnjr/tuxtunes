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
  generation: number;
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
    generation: b.generation,
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
   * Generations of batches the worker has started and not yet reported
   * complete. Both edges come from the worker's own ordered event
   * stream, never from the invoke response — a fast-failing batch can
   * finish before that response arrives. The worker emits exactly one
   * complete per started batch, cancelled or not, so this drains to
   * idle on its own.
   */
  readonly live = signal<ReadonlySet<number>>(new Set());

  readonly running = computed(this.#computeRunning.bind(this));
  /** One line for the last run: "3 converted to FLAC, 1 failed". */
  readonly summary = computed(this.#computeSummary.bind(this));

  #computeRunning(): boolean {
    return this.live().size > 0;
  }

  #computeSummary(): string {
    const c = this.lastComplete();
    if (!c) return '';
    // A run of mixed formats has no single name to give.
    const to = c.format.includes('+') ? '' : ` to ${c.format.toUpperCase()}`;
    const parts = [`${c.converted} converted${to}`];
    if (c.addedToLibrary > 0) parts.push(`${c.addedToLibrary} added to the library`);
    if (c.failed > 0) parts.push(`${c.failed} failed`);
    if (c.cancelled) parts.push(`cancelled with ${c.total - c.converted - c.failed} left`);
    return parts.join(', ');
  }

  private readonly unlisteners: UnlistenFn[] = [];
  /** Saves run one at a time so the last reply is the last write. */
  #saveChain: Promise<unknown> = Promise.resolve();

  constructor() {
    void this.subscribe();
    // Known up front, so the context menu can grey out Convert before
    // the settings tab has ever been opened.
    void this.tauri
      .invoke<boolean>('convert_available')
      .then((ok) => this.available.set(ok))
      .catch(() => null);
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribe(): Promise<void> {
    this.unlisteners.push(
      await this.tauri.listen<{ generation: number; total: number }>(
        'fs:convert-started',
        (raw) => {
          // The first batch of a run replaces the previous run's record;
          // one queued behind a live batch joins the current run.
          if (this.live().size === 0) {
            this.progress.set(null);
            this.lastComplete.set(null);
            this.failures.set([]);
          }
          this.live.update((cur) => new Set([...cur, raw.generation]));
        },
      ),
      await this.tauri.listen<ConvertProgress>('fs:convert-progress', (raw) =>
        this.progress.set(raw),
      ),
      await this.tauri.listen<{
        generation: number;
        total: number;
        converted: number;
        failed: number;
        added_to_library: number;
        cancelled: boolean;
        format: string;
      }>('fs:convert-complete', (raw) => {
        const done: ConvertComplete = {
          generation: raw.generation,
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
        this.live.update((cur) => {
          const next = new Set(cur);
          next.delete(raw.generation);
          return next;
        });
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
   * Persist prefs and resolve to what the backend stored (every knob
   * clamped). Saves are serialised: two quick edits otherwise race, and
   * whichever reply landed last would win the screen while the DB held
   * the other.
   */
  savePrefs(prefs: ConvertPrefs): Promise<ConvertPrefs> {
    const next = this.#saveChain
      .catch(() => null)
      .then(async () => {
        const stored = await this.tauri.invoke<ConvertPrefs>('set_convert_prefs', { prefs });
        this.prefs.set(stored);
        return stored;
      });
    this.#saveChain = next;
    return next;
  }

  /**
   * Queue a batch using the saved prefs. Resolves once it is queued;
   * every state change comes from the worker's events, starting with
   * `fs:convert-started`, so a rejected queue changes nothing here.
   */
  async convert(trackIds: number[], format: ConvertFormat): Promise<void> {
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
