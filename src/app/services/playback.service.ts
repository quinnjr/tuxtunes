import { Injectable, OnDestroy, computed, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { LibraryService } from './library.service';
import { TauriService } from './tauri.service';
import { UiService } from './ui.service';
import { toErrorMessage } from '../utils/errors';

export interface TrackRow {
  id: number;
  title: string;
  artist: string | null;
  album: string | null;
  durationMs: number;
  filePath: string;
  sampleRate: number | null;
  bitDepth: number | null;
  kind: string | null;
  playCount: number;
  skipCount: number;
  /** File unreachable (verify or a failed play flagged it). */
  missing: boolean;
  /** Cached cover image path, once resolved for the album. */
  artworkPath: string | null;
}

export interface TrackRowRaw {
  id: number;
  title: string;
  artist: string | null;
  album: string | null;
  duration_ms: number;
  file_path: string;
  sample_rate: number | null;
  bit_depth: number | null;
  kind: string | null;
  play_count: number;
  skip_count: number;
  import_status?: string;
  artwork_path?: string | null;
}

export function mapTrack(raw: TrackRowRaw): TrackRow {
  return {
    id: raw.id,
    title: raw.title,
    artist: raw.artist,
    album: raw.album,
    durationMs: raw.duration_ms,
    filePath: raw.file_path,
    sampleRate: raw.sample_rate,
    bitDepth: raw.bit_depth,
    kind: raw.kind,
    playCount: raw.play_count,
    skipCount: raw.skip_count,
    missing: raw.import_status === 'missing_source',
    artworkPath: raw.artwork_path ?? null,
  };
}

export type PlaybackState = 'playing' | 'paused' | 'stopped' | 'loading';

@Injectable({ providedIn: 'root' })
export class PlaybackService implements OnDestroy {
  private readonly tauri = inject(TauriService);
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);

  readonly currentTrackId = signal<number | null>(null);
  readonly state = signal<PlaybackState>('stopped');
  readonly positionMs = signal<number>(0);
  readonly durationMs = signal<number>(0);
  readonly volume = signal<number>(100);

  /**
   * The track that played immediately before the current one, as
   * reported by the engine on each `playback:track-changed` event.
   * Backs `previous()` — restart-vs-go-back semantics.
   */
  readonly previousTrackId = signal<number | null>(null);

  /**
   * Up-next queue. Plain TrackRow[] so the Now Playing panel can render
   * full metadata without re-fetching. Owned by the frontend; the
   * engine plays whatever play() is invoked with.
   */
  readonly queue = signal<TrackRow[]>([]);

  /**
   * Last user-facing failure (e.g. "File not found"). Shared with the
   * rest of the UI through UiService; kept here as an alias so
   * playback callers and specs read it from the service they hold.
   */
  readonly lastError = this.ui.lastError;

  private readonly unlisteners: UnlistenFn[] = [];

  constructor() {
    void this.subscribeEvents();
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribeEvents(): Promise<void> {
    this.unlisteners.push(
      await this.tauri.listen<{ track_id: number | null; prev_track_id: number | null }>(
        'playback:track-changed',
        (payload) => {
          this.currentTrackId.set(payload.track_id);
          this.previousTrackId.set(payload.prev_track_id);
          if (payload.track_id !== null) void this.ensureArtwork(payload.track_id);
        },
      ),
      await this.tauri.listen<{ state: PlaybackState }>('playback:state-changed', (payload) =>
        this.state.set(payload.state),
      ),
      await this.tauri.listen<{ position_ms: number; duration_ms: number }>(
        'playback:position-update',
        (payload) => {
          this.positionMs.set(payload.position_ms);
          if (payload.duration_ms > 0) this.durationMs.set(payload.duration_ms);
        },
      ),
      await this.tauri.listen<{ volume: number }>('playback:volume-changed', (payload) =>
        this.volume.set(payload.volume),
      ),
      // Auto-advance only fires for natural EOF — the engine
      // distinguishes user-stop / shutdown / redirect upstream and
      // doesn't emit `track-ended` for those.
      await this.tauri.listen<{ track_id: number }>('playback:track-ended', (payload) => {
        void this.next(payload.track_id);
      }),
      // Tray menu actions route through the frontend so the
      // state-machine logic (toggle on current state, advance from
      // queue) stays in one place.
      await this.tauri.listen('tray:toggle-play', () => void this.togglePlay()),
      await this.tauri.listen('tray:next', () => void this.next()),
      // MPRIS clients (gnome-shell, KDE plasma media controller, lock
      // screen, media keys) call into the Rust D-Bus server which
      // emits these events. They go through the same state-machine
      // path as the tray and the on-screen controls.
      await this.tauri.listen('mpris:play-pause', () => void this.togglePlay()),
      await this.tauri.listen('mpris:play', () => void this.resume()),
      await this.tauri.listen('mpris:pause', () => void this.pause()),
      await this.tauri.listen('mpris:stop', () => void this.stop()),
      await this.tauri.listen('mpris:next', () => void this.next()),
      await this.tauri.listen('mpris:previous', () => void this.previous()),
      await this.tauri.listen<number>('mpris:seek', (offsetUs) => {
        // MPRIS Seek is relative-microseconds; engine seeks absolute ms.
        void this.seek(this.positionMs() + Math.round(offsetUs / 1000));
      }),
      await this.tauri.listen<number>('mpris:set-position', (positionUs) => {
        void this.seek(Math.round(positionUs / 1000));
      }),
      await this.tauri.listen<number>('mpris:set-volume', (volumePct) => {
        void this.setVolume(volumePct);
      }),
    );
  }

  /** State-aware play/pause — used by both the transport bar and the tray. */
  async togglePlay(): Promise<void> {
    switch (this.state()) {
      // `loading` means a file is being started — audio follows within
      // milliseconds, so the user's intent is "pause".
      case 'playing':
      case 'loading': {
        await this.pause();
        break;
      }
      case 'paused': {
        await this.resume();
        break;
      }
      default: {
        break;
      }
    }
  }

  async play(trackId: number): Promise<void> {
    await this.tryPlay(trackId);
  }

  /**
   * Whether a track is audibly active — `loading` is the sub-second
   * window between the click and mpv's FileLoaded, and should already
   * present as "pause-able".
   */
  readonly isActive = computed(this.#computeIsActive.bind(this));

  /** Cover for the current track, via the library's cached rows. */
  readonly currentArtworkPath = computed(this.#computeCurrentArtworkPath.bind(this));

  #computeIsActive(): boolean {
    const s = this.state();
    return s === 'playing' || s === 'loading';
  }

  #computeCurrentArtworkPath(): string | null {
    const id = this.currentTrackId();
    if (id === null) return null;
    return this.library.tracksById().get(id)?.artworkPath ?? null;
  }

  /** Kick off a cover lookup for a track that has none cached yet. */
  private async ensureArtwork(trackId: number): Promise<void> {
    const row = this.library.tracksById().get(trackId);
    if (row?.artworkPath) return;
    try {
      await this.library.resolveTrackArtwork(trackId);
    } catch {
      // Artwork is decorative; never surface lookup failures.
    }
  }

  /**
   * Mirror the backend's `missing_source` flag onto the loaded rows so
   * the track list dims the row without a full refetch.
   */
  private markMissing(trackId: number, message: string): void {
    if (!message.startsWith('File not found')) return;
    this.library.tracks.update((rows) =>
      rows.map((t) => (t.id === trackId && !t.missing ? { ...t, missing: true } : t)),
    );
    this.queue.update((rows) =>
      rows.map((t) => (t.id === trackId && !t.missing ? { ...t, missing: true } : t)),
    );
  }

  // Transport controls never throw: a failed engine call is reported
  // through UiService so buttons, tray and MPRIS callers stay simple.
  async pause(): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('pause'));
  }

  async resume(): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('resume'));
  }

  async stop(): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('stop'));
  }

  async seek(positionMs: number): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('seek', { positionMs }));
  }

  async setVolume(volume: number): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('set_volume', { volume }));
  }

  enqueue(track: TrackRow): void {
    this.queue.update((q) => [...q, track]);
  }

  playNext(track: TrackRow): void {
    this.queue.update((q) => [track, ...q]);
  }

  removeFromQueue(index: number): void {
    this.queue.update((q) => q.filter((_, i) => i !== index));
  }

  reorderQueue(fromIndex: number, toIndex: number): void {
    this.queue.update((q) => {
      // Out-of-range indices would splice `undefined` into the queue.
      if (fromIndex < 0 || fromIndex >= q.length || toIndex < 0 || toIndex >= q.length) {
        return q;
      }
      const next = [...q];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      return next;
    });
  }

  /** Pop the head of the queue and start playing it. */
  async advanceFromQueue(): Promise<TrackRow | null> {
    const q = this.queue();
    if (q.length === 0) return null;
    const [head, ...rest] = q;
    this.queue.set(rest);
    await this.play(head.id);
    return head;
  }

  /**
   * "Next": the queue head if any, otherwise the row after `afterId`
   * (default: the current track) in the list on screen (All Songs, or
   * the open playlist). Rows flagged missing are skipped, and a row
   * that fails to start (file gone but not yet flagged) is skipped too,
   * so one bad file never stops the playlist. Returns the track that
   * started, or null at the end of the list.
   *
   * `afterId` exists because the engine sends `track-changed: null`
   * right after `track-ended`; by the time an awaited step resumes,
   * `currentTrackId` may already be null.
   */
  async next(afterId: number | null = this.currentTrackId()): Promise<TrackRow | null> {
    const fromQueue = await this.advanceFromQueue();
    if (fromQueue !== null) return fromQueue;
    const rows = this.library.tracks();
    const start = afterId === null ? -1 : rows.findIndex((t) => t.id === afterId);
    if (start === -1 && afterId !== null) return null;
    for (const candidate of rows.slice(start + 1)) {
      if (candidate.missing) continue;
      if (await this.tryPlay(candidate.id)) return candidate;
    }
    return null;
  }

  /** play(), reporting whether the engine accepted the track. */
  private async tryPlay(trackId: number): Promise<boolean> {
    try {
      await this.tauri.invoke<void>('play_track', { trackId });
      this.ui.clearError();
      return true;
    } catch (error) {
      const message = toErrorMessage(error);
      this.ui.reportError(message);
      this.markMissing(trackId, message);
      return false;
    }
  }

  clearQueue(): void {
    this.queue.set([]);
  }

  /**
   * Standard player semantics: past the 3-second grace window, "previous"
   * restarts the current track rather than hopping back a track. Within
   * the window, it goes back to whatever the engine last reported via
   * `previousTrackId` (fed by `playback:track-changed`'s `prev_track_id`);
   * with no such track, it falls back to restarting.
   */
  async previous(): Promise<void> {
    const prevId = this.previousTrackId();
    if (this.positionMs() <= 3000 && prevId !== null) {
      await this.play(prevId);
      return;
    }
    await this.seek(0);
  }
}
