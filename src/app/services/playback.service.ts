import { Injectable, OnDestroy, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';

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
  };
}

export type PlaybackState = 'playing' | 'paused' | 'stopped' | 'loading';

@Injectable({ providedIn: 'root' })
export class PlaybackService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  readonly currentTrackId = signal<number | null>(null);
  readonly state = signal<PlaybackState>('stopped');
  readonly positionMs = signal<number>(0);
  readonly durationMs = signal<number>(0);
  readonly volume = signal<number>(100);

  /**
   * Up-next queue. Plain TrackRow[] so the Now Playing panel can render
   * full metadata without re-fetching. Owned by the frontend; the
   * engine plays whatever play() is invoked with.
   */
  readonly queue = signal<TrackRow[]>([]);

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
        (payload) => this.currentTrackId.set(payload.track_id),
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
      await this.tauri.listen<{ track_id: number }>('playback:track-ended', () => {
        void this.advanceFromQueue();
      }),
      // Tray menu actions route through the frontend so the
      // state-machine logic (toggle on current state, advance from
      // queue) stays in one place.
      await this.tauri.listen('tray:toggle-play', () => void this.togglePlay()),
      await this.tauri.listen('tray:next', () => void this.advanceFromQueue()),
    );
  }

  /** State-aware play/pause — used by both the transport bar and the tray. */
  async togglePlay(): Promise<void> {
    switch (this.state()) {
      case 'playing': {
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
    await this.tauri.invoke<void>('play_track', { trackId });
  }

  async pause(): Promise<void> {
    await this.tauri.invoke<void>('pause');
  }

  async resume(): Promise<void> {
    await this.tauri.invoke<void>('resume');
  }

  async stop(): Promise<void> {
    await this.tauri.invoke<void>('stop');
  }

  async seek(positionMs: number): Promise<void> {
    await this.tauri.invoke<void>('seek', { positionMs });
  }

  async setVolume(volume: number): Promise<void> {
    await this.tauri.invoke<void>('set_volume', { volume });
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

  clearQueue(): void {
    this.queue.set([]);
  }
}
