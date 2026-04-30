export interface PathMapping {
  from: string;
  to: string;
}

export type Strategy = 'prefer_source' | 'prefer_local' | 'last_write_wins';
export type DeleteStrategy = 'respect' | 'ignore';

export interface ConflictRules {
  rating: Strategy;
  play_count: Strategy;
  skip_count: Strategy;
  last_played: Strategy;
  last_skipped: Strategy;
  loved: Strategy;
  deletes: DeleteStrategy;
}

export interface SyncSource {
  id: number;
  name: string;
  kind: string;
  sourcePath: string;
  lastSyncAt: string | null;
  lastSyncHash: string | null;
  pathMappings: PathMapping[];
  conflictRules: ConflictRules;
  autoCopyFiles: boolean;
}

interface SyncSourceRaw {
  id: number;
  name: string;
  kind: string;
  source_path: string;
  last_sync_at: string | null;
  last_sync_hash: string | null;
  path_mappings: PathMapping[];
  conflict_rules: ConflictRules;
  auto_copy_files: boolean;
}

export function mapSource(r: SyncSourceRaw): SyncSource {
  return {
    id: r.id,
    name: r.name,
    kind: r.kind,
    sourcePath: r.source_path,
    lastSyncAt: r.last_sync_at,
    lastSyncHash: r.last_sync_hash,
    pathMappings: r.path_mappings,
    conflictRules: r.conflict_rules,
    autoCopyFiles: r.auto_copy_files,
  };
}

export type { SyncSourceRaw };

export type SyncPhase =
  | 'decoding'
  | 'path_remapping'
  | 'diffing'
  | 'applying_tracks'
  | 'applying_playlists'
  | 'finalizing';

export interface SyncProgress {
  sourceId: number;
  phase: SyncPhase;
  current: number;
  total: number;
  message: string;
}

export interface SyncComplete {
  sourceId: number;
  insertedTracks: number;
  updatedTracks: number;
  deletedTracks: number;
  insertedPlaylists: number;
  updatedPlaylists: number;
  deletedPlaylists: number;
}

export interface SyncFailed {
  sourceId: number;
  error: string;
}

export type WarningKind =
  | 'missing_source_file'
  | 'unmappable_path'
  | 'smart_rule_decode_failed'
  | 'conflict_resolved'
  | 'unknown_field';

export interface SyncWarning {
  sourceId: number;
  kind: WarningKind;
  detail: string;
}
