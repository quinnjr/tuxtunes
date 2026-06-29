//! Integration: a file written by `ImportLog` is streamed line-by-line by
//! `LogTailer` — the real production pairing, without Tauri or a fixture.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tuxtunes::sync::import_log::{ImportLog, LogLevel};
use tuxtunes::sync::log_tailer::LogTailer;

#[tokio::test]
async fn tailer_streams_import_log_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let log = ImportLog::create_in_dir(tmp.path(), 1).unwrap();

    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let g = Arc::clone(&got);
    let tailer = LogTailer::spawn(log.path().to_path_buf(), move |_seq, line| {
        g.lock().unwrap().push(line);
    });

    log.write(LogLevel::Info, "reading .itl");
    log.write(LogLevel::Info, "applying tracks: 100");
    tokio::time::sleep(Duration::from_millis(400)).await;
    log.write(LogLevel::Info, "done");
    tailer.stop().await;

    let lines = got.lock().unwrap().clone();
    assert_eq!(lines.len(), 3, "got {lines:?}");
    assert!(lines[0].contains("INFO  reading .itl"));
    assert!(lines[2].contains("INFO  done"));
}
