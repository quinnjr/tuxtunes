import { Injectable, inject, signal } from '@angular/core';
import { TauriService } from './tauri.service';
import { mapTrack, TrackRow } from './playback.service';

export interface LibraryStats {
  trackCount: number;
  totalDurationMs: number;
  totalSizeBytes: number;
}

interface LibraryStatsRaw {
  track_count: number;
  total_duration_ms: number;
  total_size_bytes: number;
}

interface TrackRowRaw {
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

@Injectable({ providedIn: 'root' })
export class LibraryService {
  private readonly tauri = inject(TauriService);

  readonly stats = signal<LibraryStats | null>(null);
  readonly tracks = signal<TrackRow[]>([]);

  async refreshStats(): Promise<void> {
    const raw = await this.tauri.invoke<LibraryStatsRaw>('get_library_stats');
    this.stats.set({
      trackCount: raw.track_count,
      totalDurationMs: raw.total_duration_ms,
      totalSizeBytes: raw.total_size_bytes,
    });
  }

  async refreshTracks(limit = 500, offset = 0): Promise<void> {
    const raws = await this.tauri.invoke<TrackRowRaw[]>('list_tracks', { limit, offset });
    this.tracks.set(raws.map((raw) => mapTrack(raw)));
  }

  async addTrackFromPicker(): Promise<TrackRow | null> {
    const raw = await this.tauri.invoke<TrackRowRaw | null>('pick_and_add_track');
    if (!raw) return null;
    const mapped = mapTrack(raw);
    this.tracks.update((cur) => [mapped, ...cur]);
    await this.refreshStats();
    return mapped;
  }
}
