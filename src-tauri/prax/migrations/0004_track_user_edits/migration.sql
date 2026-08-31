-- Track metadata the user edited locally must survive a sync:
-- `user_edited` marks rows whose descriptive fields (title, artist,
-- album, album_artist, genre, year, track/disc numbers) the reconciler
-- must no longer overwrite from the source library.

ALTER TABLE "tracks" ADD COLUMN "user_edited" INTEGER NOT NULL DEFAULT 0;
