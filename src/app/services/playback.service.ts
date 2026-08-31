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
  /**
   * Album-level artist, distinct from the per-track `artist` (e.g.
   * "Various Artists" compilations). Falls back to `artist` server-side
   * when absent — see `src-tauri/src/db/tracks.rs`.
   */
  albumArtist: string | null;
  genre: string | null;
  year: number | null;
  trackNumber: number | null;
  discNumber: number | null;
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
  album_artist?: string | null;
  genre?: string | null;
  year?: number | null;
  track_number?: number | null;
  disc_number?: number | null;
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
    albumArtist: raw.album_artist ?? null,
    genre: raw.genre ?? null,
    year: raw.year ?? null,
    trackNumber: raw.track_number ?? null,
    discNumber: raw.disc_number ?? null,
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

export interface CurrentDevice {
  deviceId: string | null;
  sampleRate: number | null;
  bitDepth: number | null;
  exclusive: boolean;
}

/** `WarningKind` variants from `src-tauri/src/playback/events.rs`, snake_case on the wire. */
const WARNING_LABELS: Record<string, string> = {
  dsd_downgraded: 'DSD downgraded',
  exclusive_mode_failed: 'Exclusive mode failed',
  sample_rate_mismatch: 'Sample rate mismatch',
  load_failed: 'Load failed',
};

function describeWarning(kind: string, detail: string): string {
  return `${WARNING_LABELS[kind] ?? kind}: ${detail}`;
}

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
   * Output device the engine last reported via `playback:device-changed`
   * (e.g. after a hardware switch or an exclusive-mode handoff).
   */
  readonly currentDevice = signal<CurrentDevice | null>(null);

  /**
   * Artwork resolved for a track not present in `library.tracks` (queue /
   * album-grid playback). Written by `ensureArtwork` on success and
   * consulted first so a repeat `track-changed` for the same id doesn't
   * re-invoke the backend. `#computeCurrentArtworkPath` falls back to it
   * when the row isn't loaded into the library.
   */
  private readonly resolvedArtwork = signal<{ id: number; path: string } | null>(null);

  /**
   * Last user-facing failure (e.g. "File not found"). Shared with the
   * rest of the UI through UiService; kept here as an alias so
   * playback callers and specs read it from the service they hold.
   */
  readonly lastError = this.ui.lastError;

  private readonly unlisteners: UnlistenFn[] = [];

  /**
   * Generation counter for user-initiated playback starts. Bumped by
   * `play()`, `next()`, `previous()` and `advanceFromQueue()` so an
   * in-flight `next()` skip-walk (see below) can detect that a newer
   * start superseded it and abort instead of racing to play a stale
   * candidate over whatever the user just started.
   */
  private playSeq = 0;

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
          if (payload.track_id !== null) {
            void this.ensureArtwork(payload.track_id);
            void this.prefetchNext(payload.track_id);
          }
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
      await this.tauri.listen<{ kind: string; detail: string }>('playback:warning', (payload) =>
        this.ui.reportError(describeWarning(payload.kind, payload.detail)),
      ),
      await this.tauri.listen<{
        device_id: string | null;
        sample_rate: number | null;
        bit_depth: number | null;
        exclusive: boolean;
      }>('playback:device-changed', (payload) =>
        this.currentDevice.set({
          deviceId: payload.device_id,
          sampleRate: payload.sample_rate,
          bitDepth: payload.bit_depth,
          exclusive: payload.exclusive,
        }),
      ),
      // Auto-advance only fires for natural EOF — the engine
      // distinguishes user-stop / shutdown / redirect upstream and
      // doesn't emit `track-ended` for those.
      await this.tauri.listen<{ track_id: number; next_track_id?: number | null }>(
        'playback:track-ended',
        (payload) => {
          // Gapless: the engine already rolled into the pre-queued
          // track; just settle our bookkeeping.
          if (payload.next_track_id != null) {
            this.onPrefetchedStarted(payload.next_track_id);
            return;
          }
          this.prefetched = null;
          void this.next(payload.track_id);
        },
      ),
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

    // The engine restores the persisted volume before this listener
    // exists and de-duplicates repeats, so the slider would sit at its
    // default until the user touches it. Read the persisted value once.
    await this.syncVolumeFromPrefs();
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
    this.playSeq++;
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
    const fromLibrary = this.library.tracksById().get(id)?.artworkPath ?? null;
    if (fromLibrary !== null) return fromLibrary;
    const resolved = this.resolvedArtwork();
    return resolved !== null && resolved.id === id ? resolved.path : null;
  }

  /**
   * Kick off a cover lookup for a track that has none cached yet — either
   * in `library.tracks` or in `resolvedArtwork` (rows played from the
   * queue / album grid never land in `library.tracks`).
   */
  private async ensureArtwork(trackId: number): Promise<void> {
    const row = this.library.tracksById().get(trackId);
    if (row?.artworkPath) return;
    const cached = this.resolvedArtwork();
    if (cached !== null && cached.id === trackId) return;
    try {
      const path = await this.library.resolveTrackArtwork(trackId);
      if (path !== null) this.resolvedArtwork.set({ id: trackId, path });
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

  private async syncVolumeFromPrefs(): Promise<void> {
    try {
      const prefs = await this.tauri.invoke<{ volume?: number | null } | null>('get_audio_prefs');
      const v = prefs?.volume;
      if (typeof v === 'number' && Number.isFinite(v))
        this.volume.set(Math.min(100, Math.max(0, v)));
    } catch {
      // Cosmetic: keep the default until the next volume event.
    }
  }

  async setVolume(volume: number): Promise<void> {
    await this.ui.guard(this.tauri.invoke<void>('set_volume', { volume }));
  }

  enqueue(track: TrackRow): void {
    this.queue.update((q) => [...q, track]);
    this.refreshPrefetch();
  }

  playNext(track: TrackRow): void {
    this.queue.update((q) => [track, ...q]);
    this.refreshPrefetch();
  }

  removeFromQueue(index: number): void {
    this.queue.update((q) => q.filter((_, i) => i !== index));
    this.refreshPrefetch();
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
    this.refreshPrefetch();
  }

  /**
   * Pop the head of the queue and start playing it. Keeps popping and
   * discarding on failure — via `tryPlay`, which reports and flags the
   * track itself — until one starts or the queue drains, so a single
   * bad queued file doesn't get reported as "started" and stall
   * `next()` before it falls through to the on-screen list.
   */
  async advanceFromQueue(): Promise<TrackRow | null> {
    this.playSeq++;
    for (;;) {
      const q = this.queue();
      if (q.length === 0) return null;
      const [head, ...rest] = q;
      this.queue.set(rest);
      if (await this.tryPlay(head.id)) return head;
    }
  }

  /** What the engine has pre-queued behind the current track, if anything. */
  private prefetched: { id: number; fromQueue: boolean } | null = null;

  /**
   * The track that would follow `afterId`: the queue head, else the
   * next non-missing row in the visible list. Pure — no playback.
   */
  private nextCandidate(afterId: number): { id: number; fromQueue: boolean } | null {
    const head = this.queue().find((t) => !t.missing);
    if (head) return { id: head.id, fromQueue: true };
    const rows = this.library.tracks();
    const start = rows.findIndex((t) => t.id === afterId);
    if (start === -1) return null;
    const row = rows.slice(start + 1).find((t) => !t.missing);
    return row ? { id: row.id, fromQueue: false } : null;
  }

  /**
   * Hand the engine the next track so EOF switches gaplessly instead
   * of stopping, reopening the audio device and waiting for us. A
   * failed prefetch (missing file) just leaves the normal EOF path.
   */
  private async prefetchNext(afterId: number): Promise<void> {
    const cand = this.nextCandidate(afterId);
    try {
      if (cand === null) {
        if (this.prefetched !== null) await this.tauri.invoke<void>('clear_prefetch');
        this.prefetched = null;
        return;
      }
      if (this.prefetched?.id === cand.id) return;
      await this.tauri.invoke<void>('prefetch_next', { trackId: cand.id });
      this.prefetched = cand;
    } catch {
      this.prefetched = null;
    }
  }

  /** Re-evaluate the pre-queued track after the queue changed. */
  private refreshPrefetch(): void {
    const current = this.currentTrackId();
    if (current !== null) void this.prefetchNext(current);
  }

  private onPrefetchedStarted(nextId: number): void {
    if (this.prefetched?.fromQueue && this.prefetched.id === nextId) {
      this.queue.update((q) => {
        const i = q.findIndex((t) => t.id === nextId);
        return i === -1 ? q : q.filter((_, idx) => idx !== i);
      });
    }
    this.prefetched = null;
  }

  /**
   * "Next": the queue head if any, otherwise the row after `afterId`
   * (default: the current track) in the list on screen (All Songs, or
   * the open playlist). Rows flagged missing are skipped, and a row
   * that fails to start (file gone but not yet flagged) is skipped too,
   * so one bad file never stops the playlist. Returns the track that
   * started, or null at the end of the list.
   *
   * The skip-walk is bounded (25 consecutive failures) and cancellable:
   * `playSeq` is snapshotted once `advanceFromQueue()` has settled, and
   * re-checked after every awaited `tryPlay` so a walk grinding through
   * an unmounted drive backs off the moment a newer user-initiated start
   * (`play()`, `next()`, `previous()`, `advanceFromQueue()`) supersedes
   * it, rather than racing an IPC call + DB write + list re-render per
   * row against whatever the user just started.
   *
   * `afterId` exists because the engine sends `track-changed: null`
   * right after `track-ended`; by the time an awaited step resumes,
   * `currentTrackId` may already be null.
   */
  async next(afterId: number | null = this.currentTrackId()): Promise<TrackRow | null> {
    const fromQueue = await this.advanceFromQueue();
    if (fromQueue !== null) return fromQueue;
    const seq = this.playSeq;
    const rows = this.library.tracks();
    const start = afterId === null ? -1 : rows.findIndex((t) => t.id === afterId);
    if (start === -1 && afterId !== null) return null;
    let failCount = 0;
    for (const candidate of rows.slice(start + 1)) {
      if (candidate.missing) continue;
      if (seq !== this.playSeq) return null;
      const started = await this.tryPlay(candidate.id);
      if (seq !== this.playSeq) return null;
      if (started) return candidate;
      failCount++;
      if (failCount >= 25) {
        this.ui.reportError(`Stopped: ${failCount} tracks in a row could not be played`);
        return null;
      }
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
    this.refreshPrefetch();
  }

  /**
   * Standard player semantics: past the 3-second grace window, "previous"
   * restarts the current track rather than hopping back a track. Within
   * the window, it goes back to whatever the engine last reported via
   * `previousTrackId` (fed by `playback:track-changed`'s `prev_track_id`);
   * with no such track, it falls back to restarting.
   */
  async previous(): Promise<void> {
    this.playSeq++;
    const prevId = this.previousTrackId();
    if (this.positionMs() <= 3000 && prevId !== null) {
      await this.play(prevId);
      return;
    }
    await this.seek(0);
  }
}
