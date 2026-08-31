-- Outbound device sync: known devices, and the manifest of what
-- TuxTunes has written to each one.
--
-- `device_objects` is the sole authority for pruning. TuxTunes never
-- deletes a device path it has no row for, so files the user put on the
-- device by hand — or another app's data — are never touched.
--
-- `mount_path` is set only for `kind = 'filesystem'` devices, where the
-- transport needs a host path to resolve against.

CREATE TABLE "devices" (
    "id"                     INTEGER PRIMARY KEY AUTOINCREMENT,
    "name"                   TEXT NOT NULL,
    "kind"                   TEXT NOT NULL DEFAULT 'filesystem',
    "device_key"             TEXT NOT NULL UNIQUE,
    "key_is_weak"            INTEGER NOT NULL DEFAULT 0,
    "root_path"              TEXT NOT NULL DEFAULT '/Music',
    "mount_path"             TEXT,
    "last_seen_at"           TEXT,
    "last_sync_at"           TEXT,
    "selection"              TEXT NOT NULL DEFAULT '[]',
    "profile_override"       TEXT NOT NULL DEFAULT '{}',
    "transcode_policy"       TEXT NOT NULL DEFAULT '{}',
    "layout_template"        TEXT NOT NULL DEFAULT '{album_artist}/{album}/{disc}-{track} {title}.{ext}',
    "stats_cursors"          TEXT NOT NULL DEFAULT '{}',
    "conflict_rules"         TEXT NOT NULL DEFAULT '{}',
    "auto_sync"              INTEGER NOT NULL DEFAULT 0,
    "mirror_deletes"         INTEGER NOT NULL DEFAULT 1,
    "write_playlist_objects" INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE "device_objects" (
    "id"            INTEGER PRIMARY KEY AUTOINCREMENT,
    "device_id"     INTEGER NOT NULL,
    "kind"          TEXT NOT NULL,
    "track_id"      INTEGER,
    "persistent_id" TEXT,
    "device_path"   TEXT NOT NULL,
    "object_id"     TEXT,
    "source_hash"   TEXT,
    "encoded_codec" TEXT NOT NULL,
    "size_bytes"    INTEGER NOT NULL DEFAULT 0,
    "pushed_at"     TEXT NOT NULL DEFAULT (datetime('now')),
    CONSTRAINT "fk_device_objects_device_id" FOREIGN KEY ("device_id")
        REFERENCES "devices" ("id") ON DELETE CASCADE,
    CONSTRAINT "fk_device_objects_track_id" FOREIGN KEY ("track_id")
        REFERENCES "tracks" ("id") ON DELETE SET NULL
);

CREATE UNIQUE INDEX "idx_device_objects_device_id_device_path"
    ON "device_objects" ("device_id", "device_path");
CREATE INDEX "idx_device_objects_device_id_track_id"
    ON "device_objects" ("device_id", "track_id");
CREATE INDEX "idx_device_objects_device_id_kind"
    ON "device_objects" ("device_id", "kind");
