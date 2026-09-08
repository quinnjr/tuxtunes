//! Transcode library files to FLAC or M4A via `ffmpeg`.
//!
//! ponytail: shells out to ffmpeg rather than linking an encoder. It is
//! already the de-facto codec dependency on every Linux desktop, it
//! covers every input format the library can hold, and the whole
//! encoder surface reduces to an argv this module can unit-test.
//!
//! Conversion never touches the `tracks` table: the output is a new
//! file beside the source (or under `output_dir`), and the library row
//! keeps pointing at the original. Adding the result to the library is
//! the import path's job, not this one.

use crate::db::tracks::{self, TrackRow};
use crate::fs::events::{
    ConvertComplete, ConvertFailed, ConvertProgress, CONVERT_COMPLETE, CONVERT_FAILED,
    CONVERT_PROGRESS,
};
use crate::fs::ingest::IngestCommand;
use crate::fs::path;
use lofty::file::TaggedFileExt;
use lofty::picture::MimeType;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertFormat {
    Flac,
    M4a,
}

impl ConvertFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ConvertFormat::Flac => "flac",
            ConvertFormat::M4a => "m4a",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum M4aCodec {
    /// Apple Lossless — the lossless option inside an MP4 container.
    Alac,
    /// Lossy AAC, rate controlled by [`M4aPrefs::vbr`].
    Aac,
}

/// FLAC encoder settings. Defaults are the maximum-quality end of every
/// knob: level 12 (smallest file, same bit-exact audio) and the source's
/// own rate and depth, since resampling or truncating can only lose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlacPrefs {
    /// 0–12. Compression only — every level decodes bit-identically.
    pub compression_level: u8,
    /// Output sample rate in Hz; `None` keeps the source's.
    pub sample_rate: Option<u32>,
    /// Output bit depth (16, 24 or 32); `None` keeps the source's.
    pub bit_depth: Option<u8>,
}

impl Default for FlacPrefs {
    fn default() -> Self {
        Self {
            compression_level: 12,
            sample_rate: None,
            bit_depth: None,
        }
    }
}

/// M4A settings. Defaults to ALAC so the out-of-the-box result is
/// lossless; the AAC fields only matter once the codec is switched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct M4aPrefs {
    pub codec: M4aCodec,
    /// ALAC only: output bit depth (16 or 24); `None` keeps the source's.
    pub bit_depth: Option<u8>,
    /// Output sample rate in Hz; `None` keeps the source's.
    pub sample_rate: Option<u32>,
    /// AAC only: variable rather than constant bitrate.
    pub vbr: bool,
    /// AAC VBR quality for `-q:a`. The native encoder's usable range is
    /// roughly 0.1 (worst) – 2.0 (best).
    pub vbr_quality: f32,
    /// AAC CBR target in kbit/s.
    pub bitrate_kbps: u32,
}

impl Default for M4aPrefs {
    fn default() -> Self {
        Self {
            codec: M4aCodec::Alac,
            bit_depth: None,
            sample_rate: None,
            vbr: false,
            vbr_quality: 2.0,
            bitrate_kbps: 256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvertPrefs {
    #[serde(default)]
    pub flac: FlacPrefs,
    #[serde(default)]
    pub m4a: M4aPrefs,
    /// Where converted files land. `None` writes beside the source.
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Replace an existing output instead of writing ` (2)` beside it.
    #[serde(default)]
    pub overwrite: bool,
    /// Add each converted file to the library as its own track, the
    /// same way a picked file is added (probe, insert, copy-on-add).
    #[serde(default = "default_true")]
    pub add_to_library: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ConvertPrefs {
    fn default() -> Self {
        Self {
            flac: FlacPrefs::default(),
            m4a: M4aPrefs::default(),
            output_dir: None,
            overwrite: false,
            add_to_library: true,
        }
    }
}

impl ConvertPrefs {
    /// Clamp every knob into the range its encoder actually accepts.
    /// The values arrive from the webview, so treat them as untrusted:
    /// an out-of-range `-compression_level` makes ffmpeg exit non-zero
    /// and the conversion fails for a reason the user cannot see.
    pub fn sanitized(mut self) -> Self {
        self.flac.compression_level = self.flac.compression_level.min(12);
        self.flac.bit_depth = self.flac.bit_depth.map(clamp_lossless_depth);
        self.flac.sample_rate = self.flac.sample_rate.map(clamp_sample_rate);
        // ALAC has no 32-bit mode; offering one would just fail at encode.
        self.m4a.bit_depth = self.m4a.bit_depth.map(|d| if d <= 16 { 16 } else { 24 });
        self.m4a.sample_rate = self.m4a.sample_rate.map(clamp_sample_rate);
        self.m4a.vbr_quality = self.m4a.vbr_quality.clamp(0.1, 2.0);
        self.m4a.bitrate_kbps = self.m4a.bitrate_kbps.clamp(8, 512);
        self
    }
}

fn clamp_lossless_depth(d: u8) -> u8 {
    match d {
        0..=16 => 16,
        17..=24 => 24,
        _ => 32,
    }
}

fn clamp_sample_rate(hz: u32) -> u32 {
    hz.clamp(8_000, 384_000)
}

/// `-sample_fmt` (and friends) for a lossless bit depth. Neither FLAC
/// nor ALAC has a 24-bit sample format: both take s32 and narrow it via
/// `bits_per_raw_sample`, which for ALAC also silences its "encoding as
/// 24 bits-per-sample" warning. ALAC wants the planar variants.
fn push_bit_depth(args: &mut Vec<OsString>, depth: u8, planar: bool) {
    let fmt = match (depth == 16, planar) {
        (true, false) => "s16",
        (true, true) => "s16p",
        (false, false) => "s32",
        (false, true) => "s32p",
    };
    args.extend(["-sample_fmt".into(), OsString::from(fmt)]);
    if depth == 24 {
        args.extend(["-bits_per_raw_sample".into(), OsString::from("24")]);
    }
}

/// How the source's embedded pictures travel to the output.
///
/// ffmpeg exposes cover art as a video stream, and `-c:v copy` only
/// works when the destination container speaks that picture codec.
/// FLAC and MP4 both take JPEG and PNG; a GIF cover (common on old
/// MP3 rips) makes either muxer refuse to write its header and the
/// whole conversion fails. Re-encoding to PNG is lossless and accepted
/// by both, so it is the fallback for everything that is not already
/// known-safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverPolicy {
    Copy,
    ReencodePng,
}

/// Decide [`CoverPolicy`] from the source's tags. lofty reads the same
/// APIC / PICTURE blocks ffmpeg turns into attached pictures; if it
/// cannot read the file at all, re-encoding is the choice that cannot
/// make things worse.
pub fn cover_policy(source: &Path) -> CoverPolicy {
    let Ok(tagged) = lofty::read_from_path(source) else {
        return CoverPolicy::ReencodePng;
    };
    let all_safe = tagged
        .tags()
        .iter()
        .flat_map(|t| t.pictures())
        .all(|p| matches!(p.mime_type(), Some(MimeType::Jpeg | MimeType::Png)));
    if all_safe {
        CoverPolicy::Copy
    } else {
        CoverPolicy::ReencodePng
    }
}

/// The full ffmpeg argv for one file, minus the program name.
///
/// Kept pure and separate from the spawn so the encoder settings are
/// testable without an ffmpeg on PATH.
pub fn build_args(
    source: &Path,
    target: &Path,
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    cover: CoverPolicy,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        // Machine-readable progress on stdout instead of the ANSI
        // status line, so `pump_progress` can report a percentage.
        "-nostats".into(),
        "-progress".into(),
        "pipe:1".into(),
        // The target name is already collision-resolved unless the user
        // asked to overwrite, so -y is safe and -n turns a race into a
        // clean failure rather than an interactive prompt.
        if prefs.overwrite { "-y" } else { "-n" }.into(),
        "-i".into(),
        source.into(),
        // First audio stream only; the cover art (if any) rides along as
        // an optional attached picture.
        "-map".into(),
        "0:a:0".into(),
        "-map".into(),
        "0:v?".into(),
        "-c:v".into(),
        match cover {
            CoverPolicy::Copy => "copy",
            CoverPolicy::ReencodePng => "png",
        }
        .into(),
        "-map_metadata".into(),
        "0".into(),
    ];

    match format {
        ConvertFormat::Flac => {
            args.extend(["-c:a".into(), OsString::from("flac")]);
            args.extend([
                "-compression_level".into(),
                OsString::from(prefs.flac.compression_level.to_string()),
            ]);
            if let Some(depth) = prefs.flac.bit_depth {
                push_bit_depth(&mut args, depth, false);
            }
            if let Some(rate) = prefs.flac.sample_rate {
                args.extend(["-ar".into(), OsString::from(rate.to_string())]);
            }
        }
        ConvertFormat::M4a => {
            match prefs.m4a.codec {
                M4aCodec::Alac => {
                    args.extend(["-c:a".into(), OsString::from("alac")]);
                    if let Some(depth) = prefs.m4a.bit_depth {
                        push_bit_depth(&mut args, depth, true);
                    }
                }
                M4aCodec::Aac => {
                    args.extend(["-c:a".into(), OsString::from("aac")]);
                    if prefs.m4a.vbr {
                        // One decimal: the encoder's scale is 0.1-stepped
                        // and a float's shortest repr can be far longer.
                        args.extend([
                            "-q:a".into(),
                            OsString::from(format!("{:.1}", prefs.m4a.vbr_quality)),
                        ]);
                    } else {
                        args.extend([
                            "-b:a".into(),
                            OsString::from(format!("{}k", prefs.m4a.bitrate_kbps)),
                        ]);
                    }
                }
            }
            if let Some(rate) = prefs.m4a.sample_rate {
                args.extend(["-ar".into(), OsString::from(rate.to_string())]);
            }
            // Without faststart the moov atom lands at the end of the
            // file, which makes the result slow to open over a network
            // share and unplayable while still being written.
            args.extend(["-movflags".into(), OsString::from("+faststart")]);
        }
    }

    args.push(target.into());
    args
}

/// Where the converted file for `source` should go.
pub fn target_path(source: &Path, format: ConvertFormat, prefs: &ConvertPrefs) -> PathBuf {
    let dir = prefs
        .output_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| source.parent().unwrap_or(Path::new(".")).to_path_buf());
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track");
    let ideal = dir.join(format!("{stem}.{}", format.extension()));
    if prefs.overwrite {
        ideal
    } else {
        path::resolve_collision(&ideal)
    }
}

/// Whether an `ffmpeg` we can drive is on PATH.
pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug)]
pub enum ConvertCommand {
    Tracks {
        track_ids: Vec<i64>,
        format: ConvertFormat,
        prefs: ConvertPrefs,
        /// From [`ConvertWorker::next_generation`]; see
        /// [`ConvertWorker::cancel`] for what it buys.
        generation: u64,
    },
}

pub struct ConvertWorker {
    pub tx: mpsc::UnboundedSender<ConvertCommand>,
    /// Highest batch generation that has been cancelled. A batch is
    /// cancelled when its own generation is `<=` this, which covers the
    /// batch in flight and everything queued behind it at the time of
    /// the cancel — but not a batch requested afterwards, since that one
    /// is numbered above the cancel. A plain shared flag would need a
    /// reset on the next request, and a cancel-then-reconvert click pair
    /// could reset it before the worker had looked.
    pub cancel: watch::Sender<u64>,
    last_generation: AtomicU64,
    _task: tokio::task::JoinHandle<()>,
}

impl ConvertWorker {
    /// One worker, one queue: transcoding is CPU-bound, and running a
    /// batch per right-click in parallel would only make every batch
    /// slower while starving playback of cores.
    ///
    /// `ingest_tx` is the ingest worker's own queue rather than the
    /// whole [`crate::fs::coordinator::FsCoordinator`], which would be a
    /// cycle — the coordinator owns this worker.
    pub fn spawn<R: Runtime>(
        engine: Arc<SqliteRawEngine>,
        ingest_tx: mpsc::UnboundedSender<IngestCommand>,
        app: AppHandle<R>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<ConvertCommand>();
        let (cancel, cancel_rx) = watch::channel(0u64);
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    ConvertCommand::Tracks {
                        track_ids,
                        format,
                        prefs,
                        generation,
                    } => {
                        convert_batch(
                            &engine,
                            &ingest_tx,
                            &app,
                            &track_ids,
                            format,
                            &prefs,
                            BatchCancel::new(cancel_rx.clone(), generation),
                        )
                        .await;
                    }
                }
            }
        });
        Self {
            tx,
            cancel,
            last_generation: AtomicU64::new(0),
            _task: task,
        }
    }

    /// Number for the next batch. Generations start at 1 so a fresh
    /// worker (cancelled generation 0) has nothing cancelled.
    pub fn next_generation(&self) -> u64 {
        self.last_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Cancel the batch in flight and everything queued so far.
    pub fn cancel_all(&self) -> Result<(), String> {
        self.cancel
            .send(self.last_generation.load(Ordering::Relaxed))
            .map_err(|_| "convert worker has exited".to_string())
    }
}

/// One batch's view of the cancel channel: "has my generation been
/// cancelled yet", which is what the encode loop actually asks.
pub struct BatchCancel {
    rx: watch::Receiver<u64>,
    generation: u64,
}

impl BatchCancel {
    pub fn new(rx: watch::Receiver<u64>, generation: u64) -> Self {
        Self { rx, generation }
    }

    fn is_cancelled(&self) -> bool {
        *self.rx.borrow() >= self.generation
    }

    /// Resolves once this batch is cancelled; pends forever otherwise
    /// (the caller races it against the encoder's progress stream).
    async fn cancelled(&mut self) {
        let generation = self.generation;
        // A closed channel means the worker is being torn down, which
        // is as good a reason to stop as a cancel.
        let _ = self.rx.wait_for(|c| *c >= generation).await;
    }
}

async fn convert_batch<R: Runtime>(
    engine: &SqliteRawEngine,
    ingest_tx: &mpsc::UnboundedSender<IngestCommand>,
    app: &AppHandle<R>,
    track_ids: &[i64],
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    mut cancel: BatchCancel,
) {
    let total = track_ids.len() as u64;
    let mut converted = 0u64;
    let mut failed = 0u64;
    let mut added = 0u64;
    let mut cancelled = false;

    for (idx, &track_id) in track_ids.iter().enumerate() {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let row: Option<TrackRow> = tracks::get(engine, track_id).await.ok();
        let name = row
            .as_ref()
            .map(|r| r.title.clone())
            .unwrap_or_else(|| format!("track {track_id}"));
        let emit_progress = |percent: Option<u8>| {
            let _ = app.emit(
                CONVERT_PROGRESS,
                ConvertProgress {
                    current: idx as u64,
                    total,
                    title: name.clone(),
                    percent,
                },
            );
        };
        emit_progress(Some(0));

        let outcome = match &row {
            Some(row) => convert_one(row, format, prefs, &mut cancel, &emit_progress).await,
            None => Err(anyhow::anyhow!("track is no longer in the library")),
        };
        match outcome {
            Ok(Converted::Cancelled) => {
                cancelled = true;
                break;
            }
            Ok(Converted::File(target)) => {
                converted += 1;
                emit_progress(Some(100));
                if prefs.add_to_library {
                    match add_converted_to_library(engine, ingest_tx, &target).await {
                        Ok(true) => added += 1,
                        // The file is on disk and correct either way, so
                        // a library-add problem is worth a log line but
                        // not a failed conversion.
                        Ok(false) => {}
                        Err(e) => log::warn!(
                            "converted {} but could not add it to the library: {e}",
                            target.display()
                        ),
                    }
                }
            }
            Err(e) => {
                failed += 1;
                log::warn!("convert failed for track {track_id}: {e}");
                let _ = app.emit(
                    CONVERT_FAILED,
                    ConvertFailed {
                        track_id,
                        title: name.clone(),
                        error: e.to_string(),
                    },
                );
            }
        }
    }

    let _ = app.emit(
        CONVERT_COMPLETE,
        ConvertComplete {
            total,
            converted,
            failed,
            added_to_library: added,
            cancelled,
            format: format.extension().to_string(),
        },
    );
}

/// Outcome of one file: either it was written, or the user cancelled
/// partway through. A cancel is not an error — nothing went wrong.
#[derive(Debug)]
enum Converted {
    File(PathBuf),
    Cancelled,
}

/// Probe the converted file and insert it as its own track, then queue
/// copy-on-add exactly as the file picker does. Returns whether a row
/// was actually added — a file the library already references (a
/// re-convert onto the same overwritten path) is left alone.
async fn add_converted_to_library(
    engine: &SqliteRawEngine,
    ingest_tx: &mpsc::UnboundedSender<IngestCommand>,
    target: &Path,
) -> anyhow::Result<bool> {
    if crate::library::ingest::track_id_for_path(engine, target)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let id = crate::library::ingest::probe_and_add(engine, target).await?;
    // Send rather than await: the ingest worker owns the copy, and a
    // full library root must not stall the rest of the batch.
    ingest_tx
        .send(IngestCommand::CopyForTrack {
            track_id: id,
            source_path: target.to_path_buf(),
        })
        .map_err(|_| anyhow::anyhow!("ingest worker has exited"))?;
    Ok(true)
}

async fn convert_one(
    row: &TrackRow,
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    cancel: &mut BatchCancel,
    emit_progress: &(dyn Fn(Option<u8>) + Sync),
) -> anyhow::Result<Converted> {
    let source = PathBuf::from(&row.file_path);
    if !source.exists() {
        anyhow::bail!("source file is missing: {}", source.display());
    }
    let target = target_path(&source, format, prefs);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Converting a file onto itself would truncate the input mid-read.
    if path::same_file(&source, &target) {
        anyhow::bail!("source and destination are the same file");
    }

    let args = build_args(&source, &target, format, prefs, cover_policy(&source));
    let mut child = tokio::process::Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!("ffmpeg was not found on PATH — install it to convert files")
            }
            _ => anyhow::Error::from(e),
        })?;

    // stderr is drained on its own task: `-loglevel error` normally
    // says nothing, but a chatty failure that filled the pipe while we
    // are blocked reading stdout would deadlock the child.
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        buf
    });

    let progress_cancelled =
        pump_progress(&mut child, row.duration_ms, cancel, emit_progress).await;

    if progress_cancelled {
        // SIGKILL rather than a graceful stop: ffmpeg's own -t/q flow
        // would finalise the file, and a half-length track that looks
        // complete is worse than no file at all.
        let _ = child.kill().await;
        let _ = stderr_task.await;
        let _ = tokio::fs::remove_file(&target).await;
        return Ok(Converted::Cancelled);
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();

    if !status.success() {
        // A failed encode can still have created a truncated file;
        // leaving it behind would look like a successful conversion.
        let _ = tokio::fs::remove_file(&target).await;
        let detail = stderr
            .trim()
            .lines()
            .last()
            .unwrap_or("no output")
            .to_string();
        anyhow::bail!("ffmpeg failed: {detail}");
    }
    Ok(Converted::File(target))
}

/// Read ffmpeg's `-progress` stream, emitting a percentage as it moves.
/// Returns true if the user cancelled before the stream ended.
async fn pump_progress(
    child: &mut tokio::process::Child,
    duration_ms: i64,
    cancel: &mut BatchCancel,
    emit_progress: &(dyn Fn(Option<u8>) + Sync),
) -> bool {
    let Some(stdout) = child.stdout.take() else {
        return false;
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut last_percent: Option<u8> = None;

    loop {
        tokio::select! {
            // Biased so a cancel that arrives together with a progress
            // line is acted on immediately rather than one line later.
            biased;
            _ = cancel.cancelled() => return true,
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let Some(percent) = percent_from_progress_line(&line, duration_ms) else {
                        continue;
                    };
                    // One emit per whole percent: a 3-minute track
                    // produces a progress block every few hundred ms,
                    // and the UI cannot show more than this anyway.
                    if last_percent != Some(percent) {
                        last_percent = Some(percent);
                        emit_progress(Some(percent));
                    }
                }
                // EOF or an unreadable pipe: the child is done talking,
                // let the caller reap its exit status.
                _ => return false,
            },
        }
    }
}

/// Percentage for one `key=value` line of ffmpeg's `-progress` output,
/// or `None` for every line that is not a usable time marker.
///
/// A track whose duration the library does not know (`0`) yields no
/// percentage at all rather than a made-up one.
pub fn percent_from_progress_line(line: &str, duration_ms: i64) -> Option<u8> {
    if duration_ms <= 0 {
        return None;
    }
    let us: i64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    if us < 0 {
        return None;
    }
    let percent = (us / 1000).saturating_mul(100) / duration_ms;
    Some(percent.clamp(0, 100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(format: ConvertFormat, prefs: &ConvertPrefs) -> Vec<String> {
        build_args(
            Path::new("/music/in.wav"),
            Path::new("/music/out.x"),
            format,
            prefs,
            CoverPolicy::Copy,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    fn pair(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args[i + 1].clone())
    }

    #[test]
    fn flac_defaults_are_max_quality_and_source_native() {
        let args = args_of(ConvertFormat::Flac, &ConvertPrefs::default());
        assert_eq!(pair(&args, "-c:a").as_deref(), Some("flac"));
        assert_eq!(pair(&args, "-compression_level").as_deref(), Some("12"));
        // Progress must be machine-readable or the UI has nothing to show.
        assert_eq!(pair(&args, "-progress").as_deref(), Some("pipe:1"));
        // No resample, no requantise unless asked.
        assert!(!args.iter().any(|a| a == "-ar"));
        assert!(!args.iter().any(|a| a == "-sample_fmt"));
        assert_eq!(args.last().unwrap(), "/music/out.x");
    }

    #[test]
    fn flac_24_bit_uses_s32_narrowed_by_bits_per_raw_sample() {
        let prefs = ConvertPrefs {
            flac: FlacPrefs {
                compression_level: 5,
                sample_rate: Some(96_000),
                bit_depth: Some(24),
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::Flac, &prefs);
        assert_eq!(pair(&args, "-sample_fmt").as_deref(), Some("s32"));
        assert_eq!(pair(&args, "-bits_per_raw_sample").as_deref(), Some("24"));
        assert_eq!(pair(&args, "-ar").as_deref(), Some("96000"));
    }

    #[test]
    fn m4a_defaults_to_lossless_alac() {
        let args = args_of(ConvertFormat::M4a, &ConvertPrefs::default());
        assert_eq!(pair(&args, "-c:a").as_deref(), Some("alac"));
        assert!(!args.iter().any(|a| a == "-b:a"));
        assert_eq!(pair(&args, "-movflags").as_deref(), Some("+faststart"));
    }

    #[test]
    fn alac_24_bit_is_planar_s32_narrowed_like_flac() {
        let prefs = ConvertPrefs {
            m4a: M4aPrefs {
                bit_depth: Some(24),
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::M4a, &prefs);
        assert_eq!(pair(&args, "-sample_fmt").as_deref(), Some("s32p"));
        assert_eq!(pair(&args, "-bits_per_raw_sample").as_deref(), Some("24"));

        let sixteen = ConvertPrefs {
            m4a: M4aPrefs {
                bit_depth: Some(16),
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::M4a, &sixteen);
        assert_eq!(pair(&args, "-sample_fmt").as_deref(), Some("s16p"));
        assert!(!args.iter().any(|a| a == "-bits_per_raw_sample"));
    }

    #[test]
    fn aac_picks_cbr_or_vbr_but_never_both() {
        let cbr = ConvertPrefs {
            m4a: M4aPrefs {
                codec: M4aCodec::Aac,
                bitrate_kbps: 320,
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::M4a, &cbr);
        assert_eq!(pair(&args, "-b:a").as_deref(), Some("320k"));
        assert!(!args.iter().any(|a| a == "-q:a"));

        let vbr = ConvertPrefs {
            m4a: M4aPrefs {
                codec: M4aCodec::Aac,
                vbr: true,
                vbr_quality: 1.4,
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::M4a, &vbr);
        assert_eq!(pair(&args, "-q:a").as_deref(), Some("1.4"));
        assert!(!args.iter().any(|a| a == "-b:a"));

        let default_vbr = ConvertPrefs {
            m4a: M4aPrefs {
                codec: M4aCodec::Aac,
                vbr: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::M4a, &default_vbr);
        assert_eq!(pair(&args, "-q:a").as_deref(), Some("2.0"));
    }

    #[test]
    fn cover_is_copied_or_re_encoded_to_png_per_policy() {
        let copied = args_of(ConvertFormat::Flac, &ConvertPrefs::default());
        assert_eq!(pair(&copied, "-c:v").as_deref(), Some("copy"));

        let reencoded: Vec<String> = build_args(
            Path::new("/music/in.mp3"),
            Path::new("/music/out.m4a"),
            ConvertFormat::M4a,
            &ConvertPrefs::default(),
            CoverPolicy::ReencodePng,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert_eq!(pair(&reencoded, "-c:v").as_deref(), Some("png"));
        // The picture map stays optional either way: a source with no
        // art must not fail on a missing stream.
        assert_eq!(pair(&reencoded, "-map").as_deref(), Some("0:a:0"));
        assert!(reencoded.iter().any(|a| a == "0:v?"));
    }

    #[test]
    fn an_unreadable_source_gets_the_safe_cover_policy() {
        assert_eq!(
            cover_policy(Path::new("/nonexistent/x.mp3")),
            CoverPolicy::ReencodePng
        );
    }

    #[test]
    fn gif_covers_are_re_encoded_and_jpeg_png_are_copied() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        assert_eq!(cover_policy(&src), CoverPolicy::Copy);

        attach_pictures(&src, &[MimeType::Png]);
        assert_eq!(cover_policy(&src), CoverPolicy::Copy);

        // One bad picture among good ones is enough: `-map 0:v?` takes
        // them all, and the muxer rejects the header for any one it
        // cannot write.
        attach_pictures(&src, &[MimeType::Jpeg, MimeType::Gif]);
        assert_eq!(cover_policy(&src), CoverPolicy::ReencodePng);
    }

    /// Replace the pictures on `path` with one per MIME type. The payload
    /// is always the tiny GIF: lofty stores the MIME it is told, and the
    /// end-to-end test needs bytes ffmpeg can actually decode.
    fn attach_pictures(path: &Path, mimes: &[MimeType]) {
        use lofty::config::WriteOptions;
        use lofty::picture::{Picture, PictureType};
        use lofty::tag::{Tag, TagExt, TagType};

        let mut tag = Tag::new(TagType::VorbisComments);
        for mime in mimes {
            tag.push_picture(Picture::new_unchecked(
                PictureType::CoverFront,
                Some(mime.clone()),
                None,
                TINY_GIF.to_vec(),
            ));
        }
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }

    /// A 1×1 GIF89a. ffmpeg decodes it, which is all the end-to-end
    /// test below needs from it.
    const TINY_GIF: &[u8] = &[
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
    ];

    #[test]
    fn sanitize_clamps_out_of_range_webview_input() {
        let p = ConvertPrefs {
            flac: FlacPrefs {
                compression_level: 99,
                sample_rate: Some(1),
                bit_depth: Some(20),
            },
            m4a: M4aPrefs {
                bit_depth: Some(32),
                vbr_quality: 9.0,
                bitrate_kbps: 4000,
                ..Default::default()
            },
            ..Default::default()
        };
        let p = p.sanitized();
        assert_eq!(p.flac.compression_level, 12);
        assert_eq!(p.flac.sample_rate, Some(8_000));
        assert_eq!(p.flac.bit_depth, Some(24));
        assert_eq!(p.m4a.bit_depth, Some(24));
        assert_eq!(p.m4a.vbr_quality, 2.0);
        assert_eq!(p.m4a.bitrate_kbps, 512);
    }

    /// Build a one-second test tone with ffmpeg itself, so the
    /// ffmpeg-gated tests do not need a fixture in the repo.
    fn make_source(dir: &std::path::Path) -> PathBuf {
        let src = dir.join("tone.flac");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=frequency=440:duration=1:sample_rate=44100")
            .arg(&src)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "could not build the test tone");
        src
    }

    /// A batch (generation 1) that nobody has cancelled. The sender is
    /// leaked on purpose: dropping it would read as a teardown-cancel.
    fn live_cancel() -> BatchCancel {
        let (tx, rx) = watch::channel(0u64);
        std::mem::forget(tx);
        BatchCancel::new(rx, 1)
    }

    #[test]
    fn a_cancel_covers_earlier_generations_but_not_later_ones() {
        let (tx, rx) = watch::channel(0u64);
        let first = BatchCancel::new(rx.clone(), 1);
        let second = BatchCancel::new(rx.clone(), 2);
        assert!(!first.is_cancelled());

        // Cancel issued while generation 2 is the newest batch: both
        // the in-flight and the queued batch stop...
        tx.send(2).unwrap();
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        // ...but a batch requested after the cancel is untouched, with
        // no reset needed.
        let third = BatchCancel::new(rx, 3);
        assert!(!third.is_cancelled());
    }

    fn row_for(path: &std::path::Path, duration_ms: i64) -> TrackRow {
        TrackRow {
            id: 1,
            title: "Tone".into(),
            artist: None,
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            duration_ms,
            file_path: path.display().to_string(),
            file_hash: None,
            sample_rate: None,
            bit_depth: None,
            kind: None,
            play_count: 0,
            skip_count: 0,
            import_status: "ok".into(),
            artwork_path: None,
        }
    }

    #[tokio::test]
    async fn encodes_a_real_file_and_reports_progress() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        let seen = std::sync::Mutex::new(Vec::<Option<u8>>::new());
        let mut cancel = live_cancel();

        let out = convert_one(
            &row_for(&src, 1000),
            ConvertFormat::Flac,
            &ConvertPrefs::default(),
            &mut cancel,
            &|p| seen.lock().unwrap().push(p),
        )
        .await
        .unwrap();

        let Converted::File(path) = out else {
            panic!("expected a converted file");
        };
        assert!(path.exists(), "{} was not written", path.display());
        let seen = seen.into_inner().unwrap();
        assert!(
            seen.iter().any(|p| matches!(p, Some(n) if *n > 0)),
            "no non-zero progress was reported: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_gif_cover_does_not_sink_the_conversion() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        attach_pictures(&src, &[MimeType::Gif]);
        assert_eq!(cover_policy(&src), CoverPolicy::ReencodePng);

        let mut cancel = live_cancel();
        for format in [ConvertFormat::Flac, ConvertFormat::M4a] {
            let out = convert_one(
                &row_for(&src, 1000),
                format,
                &ConvertPrefs::default(),
                &mut cancel,
                &|_| {},
            )
            .await
            .unwrap_or_else(|e| panic!("{format:?} conversion failed: {e}"));
            let Converted::File(path) = out else {
                panic!("expected a converted file");
            };
            assert!(path.exists(), "{} was not written", path.display());
        }
    }

    #[tokio::test]
    async fn a_cancelled_encode_leaves_no_partial_file() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        // Already cancelled when the encode starts: the select is
        // biased on the cancel arm, so this deterministically takes the
        // kill path rather than racing a one-second encode.
        let mut cancel = BatchCancel::new(watch::channel(1u64).1, 1);

        let out = convert_one(
            &row_for(&src, 1000),
            ConvertFormat::Flac,
            &ConvertPrefs::default(),
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap();

        assert!(matches!(out, Converted::Cancelled));
        assert!(
            !dir.path().join("tone (2).flac").exists()
                && !dir.path().join("tone.flac.part").exists(),
            "a cancelled encode left a file behind"
        );
    }

    #[tokio::test]
    async fn a_missing_source_fails_before_spawning_ffmpeg() {
        let mut cancel = live_cancel();
        let err = convert_one(
            &row_for(std::path::Path::new("/nonexistent/x.wav"), 1000),
            ConvertFormat::Flac,
            &ConvertPrefs::default(),
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("source file is missing"), "{err}");
    }

    #[test]
    fn overwrite_off_never_reuses_an_existing_name() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("song.wav");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(dir.path().join("song.flac"), b"x").unwrap();

        let prefs = ConvertPrefs::default();
        let t = target_path(&source, ConvertFormat::Flac, &prefs);
        assert_eq!(t.file_name().unwrap(), "song (2).flac");

        let overwriting = ConvertPrefs {
            overwrite: true,
            ..Default::default()
        };
        let t = target_path(&source, ConvertFormat::Flac, &overwriting);
        assert_eq!(t.file_name().unwrap(), "song.flac");
    }

    #[test]
    fn percent_needs_a_time_line_and_a_known_duration() {
        // 30s in, out of a 60s track.
        assert_eq!(
            percent_from_progress_line("out_time_us=30000000", 60_000),
            Some(50)
        );
        assert_eq!(percent_from_progress_line("out_time_us=0", 60_000), Some(0));
        // ffmpeg's last block can overshoot the container's duration.
        assert_eq!(
            percent_from_progress_line("out_time_us=61000000", 60_000),
            Some(100)
        );
        // Every other line of the progress block is not a time marker.
        assert_eq!(percent_from_progress_line("speed= 555x", 60_000), None);
        assert_eq!(percent_from_progress_line("out_time_us=N/A", 60_000), None);
        // A track the library has no duration for gets no percentage
        // rather than a fabricated one.
        assert_eq!(percent_from_progress_line("out_time_us=30000000", 0), None);
    }

    #[test]
    fn percent_does_not_overflow_on_a_long_track() {
        // (us/1000) * 100 overflows an i64 only past ~29 million hours,
        // but a saturating multiply keeps a corrupt value from wrapping
        // negative and clamping to 0%.
        assert_eq!(
            percent_from_progress_line(&format!("out_time_us={}", i64::MAX), 60_000),
            Some(100)
        );
    }

    #[test]
    fn prefs_stored_before_add_to_library_existed_default_to_adding() {
        let old = serde_json::json!({
            "flac": { "compression_level": 8, "sample_rate": null, "bit_depth": null },
            "m4a": {
                "codec": "alac", "bit_depth": null, "sample_rate": null,
                "vbr": false, "vbr_quality": 2.0, "bitrate_kbps": 256
            },
            "output_dir": null,
            "overwrite": false
        });
        let prefs: ConvertPrefs = serde_json::from_value(old).unwrap();
        assert!(prefs.add_to_library);
        assert_eq!(prefs.flac.compression_level, 8);
    }

    #[test]
    fn output_dir_redirects_the_target() {
        let prefs = ConvertPrefs {
            output_dir: Some("/exports".into()),
            overwrite: true,
            ..Default::default()
        };
        let t = target_path(Path::new("/music/a/song.wav"), ConvertFormat::M4a, &prefs);
        assert_eq!(t, Path::new("/exports/song.m4a"));
    }
}
