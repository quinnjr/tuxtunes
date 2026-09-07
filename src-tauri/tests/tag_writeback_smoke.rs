//! Writing the library's metadata back into the files, and clearing a
//! track out of the database when it is removed.

use prax_query::filter::FilterValue as FV;

/// Smallest valid PNG: an 8-bit 1x1 image.
const PNG_1PX: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0a, b'I', b'D', b'A', b'T',
    0x78, 0x9c, 0x63, 0, 1, 0, 0, 5, 0, 1, 0x0d, 0x0a, 0x2d, 0xb4, 0, 0, 0, 0, b'I', b'E', b'N',
    b'D', 0xae, 0x42, 0x60, 0x82,
];

fn write_minimal_wav(path: &std::path::Path) {
    let bytes: &[u8] = &[
        b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
        0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
        b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
    ];
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn write_tags_pushes_library_metadata_and_a_cover_into_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    let audio = tmp.path().join("song.wav");
    write_minimal_wav(&audio);
    let art = tmp.path().join("artwork").join("abc.png");
    std::fs::create_dir_all(art.parent().unwrap()).unwrap();
    std::fs::write(&art, PNG_1PX).unwrap();

    // A row whose corrections exist only here: the file has no tags at
    // all, which is exactly the iTunes-import shape.
    let id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, artist, album, album_artist, genre, year, \
             track_number, disc_number, duration_ms, size_bytes, file_path, \
             artwork_path, file_hash, playlist_ids) VALUES \
             ('Corrected Title', 'Real Artist', 'Real Album', 'Real Album Artist', \
              'Jazz', 1999, 4, 1, 100, 44, ?, ?, 'deadbeef', '[]') RETURNING id",
            &[
                FV::String(audio.display().to_string()),
                FV::String(art.display().to_string()),
            ],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let summary = tuxtunes::commands::library::write_tags_for(&db.engine, &[id]).await;
    assert_eq!(summary.written, 1, "{summary:?}");
    assert_eq!(summary.covers, 1, "{summary:?}");
    assert!(summary.failed.is_empty(), "{summary:?}");

    let tagged = lofty::read_from_path(&audio).unwrap();
    let tag = lofty::file::TaggedFileExt::primary_tag(&tagged).unwrap();
    use lofty::tag::Accessor;
    assert_eq!(tag.title().as_deref(), Some("Corrected Title"));
    assert_eq!(tag.artist().as_deref(), Some("Real Artist"));
    assert_eq!(tag.album().as_deref(), Some("Real Album"));
    assert_eq!(tag.genre().as_deref(), Some("Jazz"));
    assert_eq!(tag.year(), Some(1999));
    assert_eq!(tag.track(), Some(4));
    assert_eq!(
        tuxtunes::library::artwork::extract_embedded(&audio)
            .map(|img| img.data)
            .as_deref(),
        Some(PNG_1PX),
        "the cover the library resolved should now be in the file"
    );

    // The file's bytes changed, so the hash recorded at import is stale
    // and Verify would call the file corrupt.
    let hash: Option<String> = db
        .engine
        .raw_sql_first("SELECT file_hash FROM tracks WHERE id = ?", &[FV::Int(id)])
        .await
        .unwrap()
        .into_json()
        .get("file_hash")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    assert_eq!(hash, None, "the stale hash should be cleared");
}

#[tokio::test]
async fn write_tags_keeps_a_cover_the_file_already_has() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    let audio = tmp.path().join("song.wav");
    write_minimal_wav(&audio);
    let own = tmp.path().join("own.png");
    std::fs::write(&own, PNG_1PX).unwrap();
    tuxtunes::fs::tags::write_cover(&audio, &own).unwrap();

    // A different image in the cache. The file's own art wins: an edit
    // is no reason to replace a picture the file already carries.
    let cached = tmp.path().join("artwork").join("other.png");
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    let mut different = PNG_1PX.to_vec();
    different.extend_from_slice(&[0u8; 8]);
    std::fs::write(&cached, &different).unwrap();

    let id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
             artwork_path, playlist_ids) VALUES ('T', 100, 44, ?, ?, '[]') RETURNING id",
            &[
                FV::String(audio.display().to_string()),
                FV::String(cached.display().to_string()),
            ],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let summary = tuxtunes::commands::library::write_tags_for(&db.engine, &[id]).await;
    assert_eq!(summary.written, 1);
    assert_eq!(summary.covers, 0, "should not have replaced the file's art");
    assert_eq!(
        tuxtunes::library::artwork::extract_embedded(&audio)
            .map(|img| img.data)
            .as_deref(),
        Some(PNG_1PX)
    );
}

#[tokio::test]
async fn write_tags_reports_a_file_it_could_not_write() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    let id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('Gone', 100, 44, '/nonexistent/gone.wav', '[]') RETURNING id",
            &[],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let summary = tuxtunes::commands::library::write_tags_for(&db.engine, &[id]).await;
    assert_eq!(summary.written, 0);
    assert_eq!(summary.failed, vec!["Gone".to_string()]);
}

#[tokio::test]
async fn removing_a_track_leaves_nothing_of_it_in_the_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    // A cached cover, as $APPDATA/artwork/<hash>.png.
    let art = tmp.path().join("artwork").join("cover.png");
    std::fs::create_dir_all(art.parent().unwrap()).unwrap();
    std::fs::write(&art, PNG_1PX).unwrap();

    let insert = |title: &'static str| {
        let engine = &db.engine;
        let art = art.display().to_string();
        async move {
            engine
                .raw_sql_first(
                    "INSERT INTO tracks (title, artist, duration_ms, size_bytes, \
                     file_path, artwork_path, user_edited, playlist_ids) VALUES \
                     (?, 'Corrected Artist', 100, 44, ?, ?, 1, '[]') RETURNING id",
                    &[
                        FV::String(title.to_string()),
                        FV::String(format!("/music/{title}.flac")),
                        FV::String(art),
                    ],
                )
                .await
                .unwrap()
                .into_json()
                .get("id")
                .and_then(|v| v.as_i64())
                .unwrap()
        }
    };
    let first = insert("One").await;
    let second = insert("Two").await;

    // Two tracks share the cover, so removing one must not take it.
    tuxtunes::commands::library::remove_track_from(&db.engine, first)
        .await
        .unwrap();
    assert!(
        art.is_file(),
        "a cover another track still uses was deleted"
    );

    let left: i64 = db
        .engine
        .raw_sql_scalar(
            "SELECT COUNT(*) FROM tracks WHERE id = ?",
            &[FV::Int(first)],
        )
        .await
        .unwrap();
    assert_eq!(left, 0, "the row, and the corrections on it, must be gone");

    // The last reference takes the cached image with it.
    tuxtunes::commands::library::remove_track_from(&db.engine, second)
        .await
        .unwrap();
    assert!(!art.exists(), "the cached cover outlived its last track");

    let total: i64 = db
        .engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await
        .unwrap();
    assert_eq!(total, 0);
}

#[tokio::test]
async fn removing_a_track_leaves_a_cover_outside_the_cache_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    // A sidecar in the user's own music folder — not ours to delete.
    let sidecar = tmp.path().join("Album").join("cover.jpg");
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    std::fs::write(&sidecar, PNG_1PX).unwrap();

    let id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
             artwork_path, playlist_ids) VALUES ('T', 100, 44, '/music/a.flac', ?, '[]') \
             RETURNING id",
            &[FV::String(sidecar.display().to_string())],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    tuxtunes::commands::library::remove_track_from(&db.engine, id)
        .await
        .unwrap();
    assert!(
        sidecar.is_file(),
        "deleted a file in the user's music folder"
    );
}
