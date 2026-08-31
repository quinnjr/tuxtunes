/** How TuxTunes reaches a device. */
export type DeviceKind = 'mtp' | 'wpd' | 'filesystem';

/**
 * One thing selected for a device. Albums carry their
 * `(albumArtist, album)` pair rather than an id, because the library
 * has no albums table — only a grouping over tracks.
 */
export type SelectionEntry =
  | { kind: 'playlist'; id: number }
  | { kind: 'smart'; id: number }
  | { kind: 'album'; album_artist: string; album: string }
  | { kind: 'all' };

export interface Device {
  id: number;
  name: string;
  kind: DeviceKind;
  deviceKey: string;
  /** Pruning is suppressed: the key could match the wrong hardware. */
  keyIsWeak: boolean;
  rootPath: string;
  mountPath: string | null;
  lastSeenAt: string | null;
  lastSyncAt: string | null;
  selection: SelectionEntry[];
  layoutTemplate: string;
  autoSync: boolean;
  mirrorDeletes: boolean;
  writePlaylistObjects: boolean;
}

export interface DeviceRaw {
  id: number;
  name: string;
  kind: DeviceKind;
  device_key: string;
  key_is_weak: boolean;
  root_path: string;
  mount_path: string | null;
  last_seen_at: string | null;
  last_sync_at: string | null;
  selection: SelectionEntry[];
  layout_template: string;
  auto_sync: boolean;
  mirror_deletes: boolean;
  write_playlist_objects: boolean;
}

export function mapDevice(r: DeviceRaw): Device {
  return {
    id: r.id,
    name: r.name,
    kind: r.kind,
    deviceKey: r.device_key,
    keyIsWeak: r.key_is_weak,
    rootPath: r.root_path,
    mountPath: r.mount_path,
    lastSeenAt: r.last_seen_at,
    lastSyncAt: r.last_sync_at,
    selection: r.selection,
    layoutTemplate: r.layout_template,
    autoSync: r.auto_sync,
    mirrorDeletes: r.mirror_deletes,
    writePlaylistObjects: r.write_playlist_objects,
  };
}

/** Settings the device panel can change. */
export interface DeviceSettings {
  name: string;
  root_path: string;
  layout_template: string;
  auto_sync: boolean;
  mirror_deletes: boolean;
  write_playlist_objects: boolean;
}

export type DevicePhase =
  | 'enumerating'
  | 'planning'
  | 'transcoding'
  | 'uploading'
  | 'playlists'
  | 'pulling_stats'
  | 'pruning'
  | 'finalizing';

export interface DeviceProgress {
  deviceId: number;
  phase: DevicePhase;
  current: number;
  total: number;
  message: string;
}

export type DeviceWarningKind =
  | 'unsupported_codec'
  | 'missing_source_file'
  | 'path_truncated'
  | 'name_collision'
  | 'playlist_object_failed'
  | 'upload_failed'
  | 'delete_failed'
  | 'out_of_space';

export interface DeviceWarning {
  deviceId: number;
  kind: DeviceWarningKind;
  detail: string;
}

export interface DeviceComplete {
  deviceId: number;
  added: number;
  replaced: number;
  unchanged: number;
  deleted: number;
  playlistsWritten: number;
  skipped: number;
  bytesWritten: number;
}

export interface DeviceFailed {
  deviceId: number;
  error: string;
}

/** What a sync would do, from `preview_device_sync`. */
export interface SyncPlanSummary {
  adds: number;
  replaces: number;
  unchanged: number;
  deletes: number;
  skips: number;
  bytesOut: number;
  freeBytes: number | null;
  totalBytes: number | null;
}

export interface SyncPlanSummaryRaw {
  adds: number;
  replaces: number;
  unchanged: number;
  deletes: number;
  skips: number;
  bytes_out: number;
  free_bytes: number | null;
  total_bytes: number | null;
}

export function mapPlanSummary(r: SyncPlanSummaryRaw): SyncPlanSummary {
  return {
    adds: r.adds,
    replaces: r.replaces,
    unchanged: r.unchanged,
    deletes: r.deletes,
    skips: r.skips,
    bytesOut: r.bytes_out,
    freeBytes: r.free_bytes,
    totalBytes: r.total_bytes,
  };
}

/** Human-readable byte size, e.g. `1.4 GB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ['kB', 'MB', 'GB', 'TB'];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}
