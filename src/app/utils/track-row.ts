import { PlaybackService, TrackRow } from '../services/playback.service';

/** Tooltip explaining a dimmed row; null for healthy rows. */
export function trackRowTitle(t: Pick<TrackRow, 'missing' | 'filePath'>): string | null {
  return t.missing ? `File not found: ${t.filePath}` : null;
}

/** Whether this row is the currently playing track. */
export function isCurrentTrack(playback: PlaybackService, t: Pick<TrackRow, 'id'>): boolean {
  return playback.currentTrackId() === t.id;
}
