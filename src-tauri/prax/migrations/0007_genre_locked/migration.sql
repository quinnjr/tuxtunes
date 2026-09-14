-- `genres apply` rewrites the genre of nearly every track. Protecting
-- that with `user_edited` would also freeze title, artist, album and
-- the rest against future syncs, so the genre gets its own lock: the
-- reconciler leaves `genre` alone when either flag is set and keeps
-- updating everything else.

ALTER TABLE "tracks" ADD COLUMN "genre_locked" INTEGER NOT NULL DEFAULT 0;
