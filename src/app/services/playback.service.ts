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
    );
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
}
