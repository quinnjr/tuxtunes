-- iTunes rates albums separately from their tracks (an album record
-- with its own 0-100 rating, referenced from each track by persistent
-- id). Carry that value on every track of the album so album views can
-- sort and display it without deriving anything from track ratings.

ALTER TABLE "tracks" ADD COLUMN "album_rating" INTEGER NOT NULL DEFAULT 0;
