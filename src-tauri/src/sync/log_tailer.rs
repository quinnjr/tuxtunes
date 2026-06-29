//! Polling tailer: follows an append-only file and emits each newly
//! completed line. Robust to partial-line writes; drains on stop.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

const POLL: Duration = Duration::from_millis(150);

/// Handle to a spawned tailing task.
pub struct LogTailer {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl LogTailer {
    /// Follow `path`, calling `emit(seq, line)` for each completed line.
    /// `seq` is a monotonic counter from 0.
    pub fn spawn<F>(path: PathBuf, emit: F) -> Self
    where
        F: Fn(u64, String) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_loop = Arc::clone(&stop);
        let handle = tokio::spawn(async move {
            let mut offset: u64 = 0;
            let mut pending: Vec<u8> = Vec::new();
            let mut seq: u64 = 0;
            loop {
                let stopping = stop_loop.load(Ordering::Relaxed);
                offset = drain(&path, offset, &mut pending, &mut seq, &emit);
                if stopping {
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
        });
        Self { stop, handle }
    }

    /// Signal stop and await the task's final drain.
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

/// Read new bytes from `offset`, emit each complete line, return the new
/// offset. Partial trailing bytes stay in `pending` for the next call.
fn drain<F>(path: &Path, offset: u64, pending: &mut Vec<u8>, seq: &mut u64, emit: &F) -> u64
where
    F: Fn(u64, String),
{
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return offset,
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return offset;
    }
    let mut buf = Vec::new();
    let read = match file.read_to_end(&mut buf) {
        Ok(n) => n as u64,
        Err(_) => return offset,
    };
    pending.extend_from_slice(&buf);
    while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
        let mut line: Vec<u8> = pending.drain(..=pos).collect();
        line.pop(); // drop '\n'
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        emit(*seq, String::from_utf8_lossy(&line).into_owned());
        *seq += 1;
    }
    offset + read
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn emits_complete_lines_in_order_and_drains_on_stop() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let got = Arc::new(Mutex::new(Vec::<(u64, String)>::new()));
        let g = Arc::clone(&got);
        let tailer = LogTailer::spawn(path.clone(), move |seq, line| {
            g.lock().unwrap().push((seq, line));
        });

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "alpha").unwrap();
            writeln!(f, "beta").unwrap();
            write!(f, "gamma-partial").unwrap(); // no newline yet
            f.flush().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f).unwrap(); // finishes "gamma-partial"
            f.flush().unwrap();
        }
        tailer.stop().await; // final drain

        let lines = got.lock().unwrap().clone();
        let texts: Vec<String> = lines.iter().map(|(_, l)| l.clone()).collect();
        assert_eq!(texts, vec!["alpha", "beta", "gamma-partial"]);
        let seqs: Vec<u64> = lines.iter().map(|(s, _)| *s).collect();
        assert_eq!(seqs, vec![0, 1, 2]);
    }
}
