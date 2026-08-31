-- Local playlist edits that must survive a sync.
--
-- `name_overridden`: set when the user renames a playlist; the sync
-- reconciler preserves the local name instead of restoring the source's.
--
-- `playlist_tombstones`: (source, persistent_id) pairs the user deleted;
-- the reconciler skips these so a deleted synced playlist stays deleted.

ALTER TABLE "playlists" ADD COLUMN "name_overridden" INTEGER NOT NULL DEFAULT 0;

CREATE TABLE "playlist_tombstones" (
    "sync_source_id" INTEGER NOT NULL,
    "persistent_id" TEXT NOT NULL,
    PRIMARY KEY ("sync_source_id", "persistent_id"),
    CONSTRAINT "fk_playlist_tombstones_sync_source_id" FOREIGN KEY ("sync_source_id") REFERENCES "sync_sources" ("id") ON DELETE CASCADE
) WITHOUT ROWID;
