//! Per-run import log file writer. Best-effort: callers treat construction
//! failure as "no log" and continue the import.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
        }
    }
}

/// Borrowed logging sink threaded into the reconcilers. `Send + Sync` so the
/// enclosing reconcile future stays `Send` across its `.await` points.
pub type LogSink<'a> = &'a (dyn Fn(LogLevel, &str) + Send + Sync);

/// Append-only writer for one import run's log file.
pub struct ImportLog {
    writer: Mutex<BufWriter<File>>,
    path: PathBuf,
}

impl ImportLog {
    /// Resolve the per-app log dir (`app_log_dir()/logs`) and open a per-run
    /// file. Used in production from the sync worker.
    pub fn create<R: Runtime>(app: &AppHandle<R>, source_id: i64) -> io::Result<Self> {
        let base = app
            .path()
            .app_log_dir()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Self::create_in_dir(&base.join("logs"), source_id)
    }

    /// Testable core: ensure `dir` exists and open `import-<id>-<ts>.log`.
    pub fn create_in_dir(dir: &Path, source_id: i64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        // Colons are illegal in filenames on some platforms — use dashes.
        let ts = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S-%3f");
        let path = dir.join(format!("import-{source_id}-{ts}.log"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write one timestamped line and flush so the tailer observes it promptly.
    pub fn write(&self, level: LogLevel, msg: &str) {
        let ts = chrono::Local::now().format("%H:%M:%S%.3f");
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(w, "{ts}  {}  {msg}", level.label());
        let _ = w.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_in_dir_makes_dir_and_named_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested/logs");
        let log = ImportLog::create_in_dir(&dir, 7).unwrap();
        assert!(dir.is_dir());
        let name = log.path().file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("import-7-"), "got {name}");
        assert!(name.ends_with(".log"));
    }

    #[test]
    fn write_appends_level_and_message() {
        let tmp = tempfile::tempdir().unwrap();
        let log = ImportLog::create_in_dir(tmp.path(), 1).unwrap();
        log.write(LogLevel::Info, "hello");
        log.write(LogLevel::Warn, "careful");
        let body = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("INFO  hello"), "got {}", lines[0]);
        assert!(lines[1].contains("WARN  careful"), "got {}", lines[1]);
    }
}
