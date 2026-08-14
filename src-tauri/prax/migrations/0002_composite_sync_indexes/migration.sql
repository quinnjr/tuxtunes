-- TuxTunes composite sync-lookup index migration.
-- Hand-written: src/bin/gen-migration.rs regenerates full CREATE TABLE
-- statements for every model (it diffs from an empty baseline rather than
-- against applied migrations), which is unsafe to replay after 0001_initial.
-- gen-migration.rs now refuses to run against a non-empty prax/migrations/
-- directory unless passed --allow-existing (see its doc comment for details).
-- This migration only adds the composite indexes needed by the
-- sync-path lookups in src/db/tracks.rs (by_persistent_id) and
-- src/db/sync_util.rs (load_pid_to_local_id_map), which filter on
-- `sync_source_id = ? AND persistent_id = ?` but previously only had
-- single-column indexes to work with.

CREATE INDEX IF NOT EXISTS "idx_tracks_sync_source_id_persistent_id" ON "tracks"("sync_source_id", "persistent_id");

CREATE INDEX IF NOT EXISTS "idx_playlists_sync_source_id_persistent_id" ON "playlists"("sync_source_id", "persistent_id");
