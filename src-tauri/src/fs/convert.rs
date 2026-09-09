//! Transcode library files to FLAC or M4A via `ffmpeg`.
//!
//! ponytail: shells out to ffmpeg rather than linking an encoder. It is
//! already the de-facto codec dependency on every Linux desktop, it
//! covers every input format the library can hold, and the whole
//! encoder surface reduces to an argv this module can unit-test.
//!
//! Conversion never rewrites a library row: the output is a new file
//! beside the source (or under `output_dir`), and the source's row keeps
//! pointing at the original. With `add_to_library` the output gets a row
//! of its own, in place — it is already where the user asked for it, so
//! unlike a picked file it is not copied under the library root.

use crate::db::tracks::{self, TrackRow};
use crate::fs::events::{
    ConvertComplete, ConvertFailed, ConvertProgress, ConvertStarted, CONVERT_COMPLETE,
    CONVERT_FAILED, CONVERT_PROGRESS, CONVERT_STARTED, LIBRARY_CHANGED,
};
use crate::fs::path;
use lofty::file::TaggedFileExt;
use lofty::picture::MimeType;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

/// Every rate the MPEG-4 AAC sampling-frequency table allows. ffmpeg's
/// native encoder refuses any other explicit `-ar` outright.
const AAC_SAMPLE_RATES: [u32; 13] = [
    7_350, 8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 64_000, 88_200,
    96_000,
];

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

    /// ffmpeg muxer name. Passed explicitly because the scratch file it
    /// writes to (see [`temp_path`]) does not end in the real extension.
    /// `ipod` is what ffmpeg itself picks for `.m4a`.
    fn muxer(self) -> &'static str {
        match self {
            ConvertFormat::Flac => "flac",
            ConvertFormat::M4a => "ipod",
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
#[serde(default)]
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
#[serde(default)]
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

/// Every knob is already clamped into the range its encoder accepts:
/// deserialization goes through [`RawConvertPrefs`], so no value that
/// arrived from the webview or the preferences table can exist
/// unclamped. An out-of-range `-compression_level` would make ffmpeg
/// exit non-zero for a reason the user cannot see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawConvertPrefs")]
pub struct ConvertPrefs {
    pub flac: FlacPrefs,
    pub m4a: M4aPrefs,
    /// Where converted files land. `None` writes beside the source.
    pub output_dir: Option<String>,
    /// Replace an existing output instead of writing ` (2)` beside it.
    pub overwrite: bool,
    /// Add each converted file to the library as its own track, in
    /// place — it is not copied under the library root.
    pub add_to_library: bool,
}

/// The wire shape of [`ConvertPrefs`]: same fields, values as sent.
/// Missing fields (a blob stored before a knob existed) take defaults.
#[derive(Deserialize)]
struct RawConvertPrefs {
    #[serde(default)]
    flac: FlacPrefs,
    #[serde(default)]
    m4a: M4aPrefs,
    #[serde(default)]
    output_dir: Option<String>,
    #[serde(default)]
    overwrite: bool,
    #[serde(default = "default_true")]
    add_to_library: bool,
}

impl From<RawConvertPrefs> for ConvertPrefs {
    fn from(raw: RawConvertPrefs) -> Self {
        Self {
            flac: raw.flac,
            m4a: raw.m4a,
            output_dir: raw.output_dir,
            overwrite: raw.overwrite,
            add_to_library: raw.add_to_library,
        }
        .sanitized()
    }
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
    /// Private: every `ConvertPrefs` built from input already went
    /// through this via `From<RawConvertPrefs>`.
    fn sanitized(mut self) -> Self {
        self.flac.compression_level = self.flac.compression_level.min(12);
        self.flac.bit_depth = self.flac.bit_depth.map(clamp_lossless_depth);
        self.flac.sample_rate = self.flac.sample_rate.map(clamp_sample_rate);
        // ALAC has no 32-bit mode; offering one would just fail at encode.
        self.m4a.bit_depth = self.m4a.bit_depth.map(|d| if d <= 16 { 16 } else { 24 });
        self.m4a.sample_rate = self.m4a.sample_rate.map(match self.m4a.codec {
            M4aCodec::Alac => clamp_sample_rate,
            M4aCodec::Aac => nearest_aac_sample_rate,
        });
        self.m4a.vbr_quality = self.m4a.vbr_quality.clamp(0.1, 2.0);
        self.m4a.bitrate_kbps = self.m4a.bitrate_kbps.clamp(8, 512);
        self.output_dir = self.output_dir.as_deref().and_then(normalize_output_dir);
        self
    }
}

/// The output folder is free text. Trim it, expand a leading `~`, and
/// insist on an absolute path — a relative one would resolve against
/// whatever the process's working directory happens to be (often `/`
/// for a desktop launch) and end up in `tracks.file_path`. Anything
/// unusable falls back to "beside the source", which the settings page
/// shows because it re-seeds from what was stored.
fn normalize_output_dir(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded: PathBuf = if trimmed == "~" {
        dirs::home_dir()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    expanded
        .is_absolute()
        .then(|| expanded.display().to_string())
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

/// Snap to the closest rate the AAC encoder accepts (ties go up).
fn nearest_aac_sample_rate(hz: u32) -> u32 {
    AAC_SAMPLE_RATES
        .iter()
        .copied()
        .min_by_key(|&r| (r.abs_diff(hz), std::cmp::Reverse(r)))
        .unwrap_or(48_000)
}

/// `-sample_fmt` (and friends) for a lossless bit depth. Neither FLAC
/// nor ALAC has a 24-bit sample format: both take s32 and narrow it via
/// `bits_per_raw_sample`, which for ALAC also silences its "encoding as
/// 24 bits-per-sample" warning. ALAC wants the planar variants. A bare
/// s32 is *also* written as 24-bit by the FLAC encoder; real 32-bit
/// output needs the depth stated and the encoder's experimental gate
/// opened (ALAC never gets here with 32 — `sanitized` caps it at 24).
fn push_bit_depth(args: &mut Vec<OsString>, depth: u8, planar: bool) {
    let fmt = match (depth == 16, planar) {
        (true, false) => "s16",
        (true, true) => "s16p",
        (false, false) => "s32",
        (false, true) => "s32p",
    };
    args.extend(["-sample_fmt".into(), OsString::from(fmt)]);
    match depth {
        24 => args.extend(["-bits_per_raw_sample".into(), OsString::from("24")]),
        32 => args.extend([
            "-bits_per_raw_sample".into(),
            OsString::from("32"),
            "-strict".into(),
            OsString::from("experimental"),
        ]),
        _ => {}
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
///
/// [`CoverPolicy::Drop`] is the last resort when the picture itself is
/// broken (a tag that says JPEG over bytes that are not): the audio is
/// intact and converting it without art beats failing the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverPolicy {
    Copy,
    ReencodePng,
    Drop,
}

/// Decide [`CoverPolicy`] from the source's tags. lofty reads the same
/// APIC / PICTURE blocks ffmpeg turns into attached pictures. A source
/// with no pictures gets [`CoverPolicy::Drop`] outright: there is
/// nothing to carry, and it also tells the caller a failure cannot be
/// the cover's fault. If lofty cannot read the file at all, ffmpeg may
/// still find a picture, so re-encoding is the guess.
pub fn cover_policy(source: &Path) -> CoverPolicy {
    let Ok(tagged) = lofty::read_from_path(source) else {
        return CoverPolicy::ReencodePng;
    };
    let mut pictures = tagged.tags().iter().flat_map(|t| t.pictures()).peekable();
    if pictures.peek().is_none() {
        return CoverPolicy::Drop;
    }
    if pictures.all(|p| matches!(p.mime_type(), Some(MimeType::Jpeg | MimeType::Png))) {
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
        // `target` is this module's own scratch file (see `temp_path`),
        // never a name the user or another track owns, so -y is safe.
        "-y".into(),
        "-i".into(),
        source.into(),
        // First audio stream only.
        "-map".into(),
        "0:a:0".into(),
    ];
    match cover {
        // The cover art (if any) rides along as an optional attached
        // picture.
        CoverPolicy::Copy | CoverPolicy::ReencodePng => args.extend([
            "-map".into(),
            "0:v?".into(),
            "-c:v".into(),
            OsString::from(if cover == CoverPolicy::Copy {
                "copy"
            } else {
                "png"
            }),
        ]),
        CoverPolicy::Drop => args.push("-vn".into()),
    }
    args.extend(["-map_metadata".into(), OsString::from("0")]);

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
            // share and unplayable while still being written. Without
            // use_metadata_tags the muxer silently drops every tag
            // outside its small MP4 table — ReplayGain and MusicBrainz
            // IDs included.
            args.extend([
                "-movflags".into(),
                OsString::from("+faststart+use_metadata_tags"),
            ]);
        }
    }

    args.extend(["-f".into(), OsString::from(format.muxer())]);
    args.push(target.into());
    args
}

/// The one stderr line worth showing for a failed encode. ffmpeg logs
/// decoder grumbles first (a cover whose JPEG header is off), the real
/// cause next, then a fixed tail of generic lines about threads and
/// nothing having been written — so take the last line that is not
/// boilerplate.
fn summarize_stderr(stderr: &str) -> String {
    const GENERIC: [&str; 8] = [
        "Nothing was written",
        "Error sending frames",
        "Task finished with error",
        "Terminating thread",
        "Could not write header",
        "Could not open encoder",
        "Error while opening encoder",
        "Conversion failed",
    ];
    stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !GENERIC.iter().any(|g| l.contains(g)))
        .unwrap_or("no output")
        .to_string()
}

/// Where ffmpeg writes while it works: a dot-file beside `target` with
/// a non-audio extension, so nothing that watches the folder (external
/// change detection, other players) takes a half-written file for a
/// track. Renamed onto `target` only once ffmpeg has exited cleanly, so
/// a failure or cancel never has to delete anything at `target` — which
/// with `overwrite` may be a healthy file ffmpeg never opened.
pub fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "convert".into());
    target.with_file_name(format!(".{name}.part"))
}

/// The name the converted file for `source` would ideally take, before
/// collisions are resolved (see `resolve_target`).
pub fn target_path(source: &Path, format: ConvertFormat, prefs: &ConvertPrefs) -> PathBuf {
    let dir = prefs
        .output_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| source.parent().unwrap_or(Path::new(".")).to_path_buf());
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track");
    dir.join(format!("{stem}.{}", format.extension()))
}

/// What `resolve_target` decided about the destination.
struct Destination {
    path: PathBuf,
    /// A library row whose file *is* the destination — an earlier export
    /// of the same source, being re-done in place. Re-probed afterwards
    /// so the row does not describe bytes that no longer exist.
    refresh_row: Option<i64>,
}

/// Turn the ideal name into one that is safe to write, or refuse.
///
/// The folder is created and canonicalised first so a `.`, `..` or
/// symlinked spelling typed into the output box compares equal to the
/// paths the library stores. Without `overwrite` the name is walked to
/// one free both on disk *and* in the `tracks` table — a row whose file
/// went missing still owns its name, and landing on it would make that
/// row play unrelated audio. With `overwrite` the name is kept unless it
/// belongs to a library row: a row for the same path is a previous
/// export and is refreshed; any other match (a different track, or the
/// managed copy's `original_path`) is refused. Names claimed earlier in
/// the same batch are never reused, overwrite or not.
async fn resolve_target(
    engine: &SqliteRawEngine,
    source: &Path,
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    claimed: &mut HashSet<PathBuf>,
) -> anyhow::Result<Destination> {
    let ideal = target_path(source, format, prefs);
    let dir = ideal.parent().unwrap_or(Path::new("."));
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    let dir = tokio::fs::canonicalize(dir).await?;
    let ideal = dir.join(ideal.file_name().unwrap_or_default());

    // Converting a file onto itself would truncate the input mid-read.
    if path::same_file(source, &ideal) {
        anyhow::bail!("source and destination are the same file");
    }

    let dest = if prefs.overwrite && !claimed.contains(&ideal) {
        let refresh_row = match crate::library::ingest::track_id_for_path(engine, &ideal).await? {
            None => None,
            Some(id) => {
                let owns_it = tracks::get_opt(engine, id)
                    .await?
                    .is_some_and(|row| Path::new(&row.file_path) == ideal);
                if !owns_it {
                    anyhow::bail!(
                        "{} is a library track's file; turn off Overwrite or choose another output folder",
                        ideal.display()
                    );
                }
                Some(id)
            }
        };
        Destination {
            path: ideal,
            refresh_row,
        }
    } else {
        Destination {
            path: crate::fs::ingest::free_target(engine, &ideal, claimed).await?,
            refresh_row: None,
        }
    };
    claimed.insert(dest.path.clone());
    Ok(dest)
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
    pub fn spawn<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
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
    pub generation: u64,
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
    let mut library_changed = false;
    let mut cancelled = false;
    let mut claimed = HashSet::new();
    let _ = app.emit(
        CONVERT_STARTED,
        ConvertStarted {
            generation: cancel.generation,
            total,
        },
    );

    for (idx, &track_id) in track_ids.iter().enumerate() {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let row = tracks::get_opt(engine, track_id).await;
        let name = match &row {
            Ok(Some(r)) => r.title.clone(),
            _ => format!("track {track_id}"),
        };
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
        // No percentage at all for a track whose duration the library
        // does not know, rather than a 0% that never moves.
        let duration_known = matches!(&row, Ok(Some(r)) if r.duration_ms > 0);
        let known = |p: u8| duration_known.then_some(p);
        emit_progress(known(0));

        let outcome = match &row {
            Ok(Some(row)) => {
                convert_prepared(
                    engine,
                    row,
                    format,
                    prefs,
                    &mut cancel,
                    &emit_progress,
                    &mut claimed,
                )
                .await
            }
            Ok(None) => Err(anyhow::anyhow!("track is no longer in the library")),
            Err(e) => Err(anyhow::anyhow!(
                "could not read the track from the library: {e}"
            )),
        };
        match outcome {
            Ok(Converted::Cancelled) => {
                cancelled = true;
                break;
            }
            Ok(Converted::File { path, refresh_row }) => {
                converted += 1;
                emit_progress(known(100));
                let library = match refresh_row {
                    Some(id) => crate::library::ingest::probe_and_update(engine, id, &path)
                        .await
                        .map(|()| false),
                    None if prefs.add_to_library => {
                        crate::library::ingest::probe_and_add(engine, &path)
                            .await
                            .map(|_| true)
                    }
                    None => continue,
                };
                match library {
                    Ok(is_new) => {
                        library_changed = true;
                        if is_new {
                            added += 1;
                        }
                    }
                    // The file is on disk and correct, so this is not a
                    // failed conversion — but it is not silent either.
                    Err(e) => {
                        let _ = app.emit(
                            CONVERT_FAILED,
                            ConvertFailed {
                                track_id,
                                title: name.clone(),
                                error: format!(
                                    "converted to {} but could not add it to the library: {e}",
                                    path.display()
                                ),
                            },
                        );
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

    if library_changed {
        // Rows changed that no UI action touched; the DB watcher would
        // notice within its poll interval, this just saves the wait.
        let _ = app.emit(LIBRARY_CHANGED, ());
    }
    let _ = app.emit(
        CONVERT_COMPLETE,
        ConvertComplete {
            generation: cancel.generation,
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
    File {
        path: PathBuf,
        /// See [`Destination::refresh_row`].
        refresh_row: Option<i64>,
    },
    Cancelled,
}

/// Resolve the destination (see `resolve_target`), then convert.
async fn convert_prepared(
    engine: &SqliteRawEngine,
    row: &TrackRow,
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    cancel: &mut BatchCancel,
    emit_progress: &(dyn Fn(Option<u8>) + Sync),
    claimed: &mut HashSet<PathBuf>,
) -> anyhow::Result<Converted> {
    let source = PathBuf::from(&row.file_path);
    if !source.exists() {
        anyhow::bail!("source file is missing: {}", source.display());
    }
    let dest = resolve_target(engine, &source, format, prefs, claimed).await?;
    match convert_one(row, &dest.path, format, prefs, cancel, emit_progress).await? {
        Converted::File { path, .. } => Ok(Converted::File {
            path,
            refresh_row: dest.refresh_row,
        }),
        cancelled => Ok(cancelled),
    }
}

async fn convert_one(
    row: &TrackRow,
    target: &Path,
    format: ConvertFormat,
    prefs: &ConvertPrefs,
    cancel: &mut BatchCancel,
    emit_progress: &(dyn Fn(Option<u8>) + Sync),
) -> anyhow::Result<Converted> {
    let source = PathBuf::from(&row.file_path);
    if !source.exists() {
        anyhow::bail!("source file is missing: {}", source.display());
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!("could not create {}: {e}", parent.display()))?;
    }
    // Converting a file onto itself would truncate the input mid-read.
    if path::same_file(&source, target) {
        anyhow::bail!("source and destination are the same file");
    }
    let temp = temp_path(target);

    // lofty reads the whole tag block synchronously; keep it off the
    // shared runtime like the ingest probe.
    let mut policy = tokio::task::spawn_blocking({
        let source = source.clone();
        move || cover_policy(&source)
    })
    .await?;
    let mut first_failure: Option<String> = None;
    let outcome = loop {
        let args = build_args(&source, &temp, format, prefs, policy);
        match run_ffmpeg(&args, row.duration_ms, cancel, emit_progress).await? {
            // Only a source that actually has a picture gets a second
            // encode: with one present, the picture is what the muxer
            // chokes on far more often than the audio.
            Encode::Failed(detail) if policy != CoverPolicy::Drop => {
                log::warn!(
                    "convert of {} failed with cover art ({detail}); retrying without it",
                    source.display()
                );
                first_failure = Some(detail);
                policy = CoverPolicy::Drop;
            }
            Encode::Failed(detail) => {
                break Encode::Failed(match first_failure {
                    Some(first) if first != detail => {
                        format!("{first}; without cover art: {detail}")
                    }
                    _ => detail,
                })
            }
            other => break other,
        }
    };

    match outcome {
        Encode::Cancelled => {
            let _ = tokio::fs::remove_file(&temp).await;
            Ok(Converted::Cancelled)
        }
        Encode::Failed(detail) => {
            let _ = tokio::fs::remove_file(&temp).await;
            anyhow::bail!("ffmpeg failed: {detail}");
        }
        Encode::Done => {
            // The name was free when the batch started; a file that has
            // appeared there since is not ours to replace.
            if !prefs.overwrite && target.exists() {
                let _ = tokio::fs::remove_file(&temp).await;
                anyhow::bail!(
                    "{} appeared while converting; not overwriting it",
                    target.display()
                );
            }
            if let Err(e) = tokio::fs::rename(&temp, target).await {
                let _ = tokio::fs::remove_file(&temp).await;
                anyhow::bail!("could not move the result to {}: {e}", target.display());
            }
            Ok(Converted::File {
                path: target.to_path_buf(),
                refresh_row: None,
            })
        }
    }
}

enum Encode {
    Done,
    Cancelled,
    /// ffmpeg exited non-zero; the payload is its most useful stderr line.
    Failed(String),
}

/// Run one ffmpeg invocation to completion, or until cancelled. Only a
/// failure to launch is an `Err`; ffmpeg's own failures are data.
async fn run_ffmpeg(
    args: &[OsString],
    duration_ms: i64,
    cancel: &mut BatchCancel,
    emit_progress: &(dyn Fn(Option<u8>) + Sync),
) -> anyhow::Result<Encode> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args(args)
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

    if pump_progress(&mut child, duration_ms, cancel, emit_progress).await {
        // SIGKILL rather than a graceful stop: ffmpeg's own -t/q flow
        // would finalise the file, and a half-length track that looks
        // complete is worse than no file at all.
        let _ = child.kill().await;
        let _ = stderr_task.await;
        return Ok(Encode::Cancelled);
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if status.success() {
        return Ok(Encode::Done);
    }
    log::debug!("ffmpeg exited {status}: {stderr}");
    Ok(Encode::Failed(summarize_stderr(&stderr)))
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
        // The scratch file has no telling extension, so the muxer is
        // named outright.
        assert_eq!(pair(&args, "-f").as_deref(), Some("flac"));
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
    fn flac_32_bit_states_the_depth_and_opens_the_experimental_gate() {
        let prefs = ConvertPrefs {
            flac: FlacPrefs {
                bit_depth: Some(32),
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_of(ConvertFormat::Flac, &prefs);
        assert_eq!(pair(&args, "-sample_fmt").as_deref(), Some("s32"));
        assert_eq!(pair(&args, "-bits_per_raw_sample").as_deref(), Some("32"));
        assert_eq!(pair(&args, "-strict").as_deref(), Some("experimental"));
    }

    #[test]
    fn output_dir_is_trimmed_expanded_and_must_be_absolute() {
        let with = |dir: &str| {
            let raw = serde_json::json!({ "output_dir": dir });
            serde_json::from_value::<ConvertPrefs>(raw)
                .unwrap()
                .output_dir
        };
        assert_eq!(with("  /exports "), Some("/exports".into()));
        assert_eq!(with("   "), None);
        assert_eq!(with("relative/dir"), None);
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            with("~/Converted"),
            Some(home.join("Converted").display().to_string())
        );
        assert_eq!(with("~"), Some(home.display().to_string()));
    }

    #[test]
    fn m4a_defaults_to_lossless_alac() {
        let args = args_of(ConvertFormat::M4a, &ConvertPrefs::default());
        assert_eq!(pair(&args, "-c:a").as_deref(), Some("alac"));
        assert!(!args.iter().any(|a| a == "-b:a"));
        assert_eq!(
            pair(&args, "-movflags").as_deref(),
            Some("+faststart+use_metadata_tags")
        );
        assert_eq!(pair(&args, "-f").as_deref(), Some("ipod"));
    }

    #[test]
    fn stderr_summary_skips_decoder_noise_and_the_generic_tail() {
        let aac = "[aac @ 0x1] Specified sample rate 192000 is not supported by the aac encoder\n\
                   [out#0/ipod @ 0x2] Nothing was written into output file, because at least one of its streams received no packets.\n";
        assert!(summarize_stderr(aac).contains("192000 is not supported"));

        let gif = "[flac @ 0x1] GIF image support is not implemented.\n\
                   [out#0/flac @ 0x2] Could not write header (incorrect codec parameters ?): Not yet implemented\n\
                   [af#0:0 @ 0x3] Error sending frames to consumers: Not yet implemented\n\
                   [af#0:0 @ 0x3] Task finished with error code: -1 (Not yet implemented)\n\
                   [af#0:0 @ 0x3] Terminating thread with return code -1\n\
                   [out#0/flac @ 0x2] Nothing was written into output file\n";
        assert!(summarize_stderr(gif).contains("GIF image support"));

        // A decoder complaint about the cover precedes the real cause.
        let noisy = "[mjpeg @ 0x1] No JPEG data found in image\n\
                     [AVFormatContext @ 0x2] Unable to choose an output format for 'x.part'\n";
        assert!(summarize_stderr(noisy).contains("Unable to choose"));

        assert_eq!(summarize_stderr(""), "no output");
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
        // Nothing to carry: no picture map at all, and no retry either.
        assert_eq!(cover_policy(&src), CoverPolicy::Drop);

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
    fn aac_sample_rates_snap_to_the_mpeg4_table_but_alac_keeps_any_rate() {
        let aac = |hz: u32| ConvertPrefs {
            m4a: M4aPrefs {
                codec: M4aCodec::Aac,
                sample_rate: Some(hz),
                ..Default::default()
            },
            ..Default::default()
        };
        // 192 kHz and 176.4 kHz are offered for ALAC; AAC tops out at 96.
        assert_eq!(aac(192_000).sanitized().m4a.sample_rate, Some(96_000));
        assert_eq!(aac(176_400).sanitized().m4a.sample_rate, Some(96_000));
        assert_eq!(aac(44_100).sanitized().m4a.sample_rate, Some(44_100));
        assert_eq!(aac(50_000).sanitized().m4a.sample_rate, Some(48_000));

        let alac = ConvertPrefs {
            m4a: M4aPrefs {
                sample_rate: Some(192_000),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(alac.sanitized().m4a.sample_rate, Some(192_000));
    }

    #[test]
    fn dropping_the_cover_maps_no_video_at_all() {
        let args: Vec<String> = build_args(
            Path::new("/music/in.mp3"),
            Path::new("/music/.out.flac.part"),
            ConvertFormat::Flac,
            &ConvertPrefs::default(),
            CoverPolicy::Drop,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert!(args.iter().any(|a| a == "-vn"));
        assert!(!args.iter().any(|a| a == "0:v?"));
        assert!(!args.iter().any(|a| a == "-c:v"));
    }

    #[test]
    fn temp_path_is_a_hidden_non_audio_name_beside_the_target() {
        assert_eq!(
            temp_path(Path::new("/music/Song.flac")),
            Path::new("/music/.Song.flac.part")
        );
    }

    #[test]
    fn a_prefs_blob_missing_newer_sub_fields_still_loads() {
        let old = serde_json::json!({
            "flac": { "compression_level": 8 },
            "m4a": { "codec": "aac" }
        });
        let prefs: ConvertPrefs = serde_json::from_value(old).unwrap();
        assert_eq!(prefs.flac.compression_level, 8);
        assert_eq!(prefs.m4a.codec, M4aCodec::Aac);
        assert_eq!(prefs.m4a.bitrate_kbps, 256);
    }

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

    /// The name `resolve_target` would pick with nothing else in the way:
    /// the ideal, stepped past whatever is on disk. Tests that convert
    /// the tone to its own format need this or they hit the same-file
    /// guard.
    fn fresh_target(src: &Path, format: ConvertFormat, prefs: &ConvertPrefs) -> PathBuf {
        path::resolve_collision(&target_path(src, format, prefs))
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
            rating: 0,
            album_rating: 0,
            date_added_unix: None,
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

        let prefs = ConvertPrefs::default();
        let target = fresh_target(&src, ConvertFormat::Flac, &prefs);
        let out = convert_one(
            &row_for(&src, 1000),
            &target,
            ConvertFormat::Flac,
            &prefs,
            &mut cancel,
            &|p| seen.lock().unwrap().push(p),
        )
        .await
        .unwrap();

        let Converted::File { path, .. } = out else {
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
            let prefs = ConvertPrefs::default();
            let out = convert_one(
                &row_for(&src, 1000),
                &fresh_target(&src, format, &prefs),
                format,
                &prefs,
                &mut cancel,
                &|_| {},
            )
            .await
            .unwrap_or_else(|e| panic!("{format:?} conversion failed: {e}"));
            let Converted::File { path, .. } = out else {
                panic!("expected a converted file");
            };
            assert!(path.exists(), "{} was not written", path.display());
        }
    }

    #[tokio::test]
    async fn a_failed_overwrite_leaves_the_existing_file_untouched() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // A "source" ffmpeg cannot open, and a healthy file already at
        // the destination the overwrite would replace.
        let bad = dir.path().join("bad.mp3");
        std::fs::write(&bad, b"this is not audio").unwrap();
        let existing = dir.path().join("bad.flac");
        std::fs::write(&existing, b"precious").unwrap();

        let prefs = ConvertPrefs {
            overwrite: true,
            ..Default::default()
        };
        let target = target_path(&bad, ConvertFormat::Flac, &prefs);
        assert_eq!(target, existing);
        let mut cancel = live_cancel();
        let err = convert_one(
            &row_for(&bad, 1000),
            &target,
            ConvertFormat::Flac,
            &prefs,
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.to_string().starts_with("ffmpeg failed"), "{err}");
        assert_eq!(std::fs::read(&existing).unwrap(), b"precious");
        assert!(!temp_path(&target).exists(), "scratch file left behind");
    }

    #[tokio::test]
    async fn a_cover_whose_tag_lies_about_its_format_is_dropped_not_fatal() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        // Declared JPEG, actually GIF bytes: lofty says Copy, and both
        // Copy and PNG re-encode make ffmpeg fail on the picture.
        attach_pictures(&src, &[MimeType::Jpeg]);
        assert_eq!(cover_policy(&src), CoverPolicy::Copy);

        let prefs = ConvertPrefs::default();
        let target = target_path(&src, ConvertFormat::M4a, &prefs);
        let mut cancel = live_cancel();
        let out = convert_one(
            &row_for(&src, 1000),
            &target,
            ConvertFormat::M4a,
            &prefs,
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap();
        assert!(matches!(out, Converted::File { .. }));
        assert!(target.exists());
    }

    #[tokio::test]
    async fn overwrite_refuses_a_destination_the_library_references() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        // Another library track already lives at the name the overwrite
        // would produce.
        let other = dir.path().join("tone.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&src)
            .args(["-c:a", "alac"])
            .arg(&other)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "could not build the other track");
        let before = std::fs::metadata(&other).unwrap().len();
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::client::Db::open(tmp_db.path()).await.unwrap();
        let id = crate::library::ingest::probe_and_add(&db.engine, &other)
            .await
            .unwrap();
        // The row's managed copy lives elsewhere; `other` is what it was
        // copied from. Overwriting it would leave the copy stale.
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET file_path = ?, original_path = ? WHERE id = ?",
                &[
                    prax_query::filter::FilterValue::String("/managed/tone.m4a".into()),
                    prax_query::filter::FilterValue::String(
                        other.canonicalize().unwrap().display().to_string(),
                    ),
                    prax_query::filter::FilterValue::Int(id),
                ],
            )
            .await
            .unwrap();

        let prefs = ConvertPrefs {
            overwrite: true,
            ..Default::default()
        };
        let mut cancel = live_cancel();
        let mut claimed = HashSet::new();
        let err = convert_prepared(
            &db.engine,
            &row_for(&src, 1000),
            ConvertFormat::M4a,
            &prefs,
            &mut cancel,
            &|_| {},
            &mut claimed,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("library track's file"), "{err}");
        assert_eq!(
            std::fs::metadata(&other).unwrap().len(),
            before,
            "the other track's file was touched"
        );
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

        let prefs = ConvertPrefs::default();
        let target = fresh_target(&src, ConvertFormat::Flac, &prefs);
        let out = convert_one(
            &row_for(&src, 1000),
            &target,
            ConvertFormat::Flac,
            &prefs,
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap();

        assert!(matches!(out, Converted::Cancelled));
        assert!(
            !target.exists() && !temp_path(&target).exists(),
            "a cancelled encode left a file behind"
        );
    }

    #[tokio::test]
    async fn a_missing_source_fails_before_spawning_ffmpeg() {
        let mut cancel = live_cancel();
        let err = convert_one(
            &row_for(std::path::Path::new("/nonexistent/x.wav"), 1000),
            Path::new("/nonexistent/x.flac"),
            ConvertFormat::Flac,
            &ConvertPrefs::default(),
            &mut cancel,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("source file is missing"), "{err}");
    }

    #[tokio::test]
    async fn resolve_target_skips_names_taken_on_disk_in_the_db_or_in_this_batch() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::client::Db::open(tmp_db.path()).await.unwrap();

        // "tone.m4a" is taken on disk; "tone (2).m4a" by a row whose file
        // is gone — the app keeps such rows, and file_path is UNIQUE.
        std::fs::write(dir.path().join("tone.m4a"), b"x").unwrap();
        let ghost = dir.path().join("tone (2).m4a");
        std::fs::copy(&src, dir.path().join("ghost.flac")).unwrap();
        let id = crate::library::ingest::probe_and_add(&db.engine, &dir.path().join("ghost.flac"))
            .await
            .unwrap();
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET file_path = ? WHERE id = ?",
                &[
                    prax_query::filter::FilterValue::String(ghost.display().to_string()),
                    prax_query::filter::FilterValue::Int(id),
                ],
            )
            .await
            .unwrap();

        let prefs = ConvertPrefs::default();
        let mut claimed = HashSet::new();
        let first = resolve_target(&db.engine, &src, ConvertFormat::M4a, &prefs, &mut claimed)
            .await
            .unwrap();
        assert_eq!(first.path.file_name().unwrap(), "tone (3).m4a");
        assert!(first.refresh_row.is_none());

        // Same stem again in the same batch: the name just claimed is
        // not free even though nothing is on disk yet.
        let second = resolve_target(&db.engine, &src, ConvertFormat::M4a, &prefs, &mut claimed)
            .await
            .unwrap();
        assert_eq!(second.path.file_name().unwrap(), "tone (4).m4a");
    }

    #[tokio::test]
    async fn overwrite_dedupes_within_a_batch_and_canonicalises_the_folder() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::client::Db::open(tmp_db.path()).await.unwrap();

        // A `.` spelling of the folder must land in the same place.
        let prefs = ConvertPrefs {
            overwrite: true,
            output_dir: Some(format!("{}/.", dir.path().display())),
            ..Default::default()
        };
        let mut claimed = HashSet::new();
        let first = resolve_target(&db.engine, &src, ConvertFormat::M4a, &prefs, &mut claimed)
            .await
            .unwrap();
        assert_eq!(
            first.path,
            dir.path().canonicalize().unwrap().join("tone.m4a")
        );
        let second = resolve_target(&db.engine, &src, ConvertFormat::M4a, &prefs, &mut claimed)
            .await
            .unwrap();
        assert_eq!(second.path.file_name().unwrap(), "tone (2).m4a");
    }

    #[tokio::test]
    async fn overwrite_onto_its_own_previous_export_refreshes_that_row() {
        if !ffmpeg_available() {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_source(dir.path());
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::client::Db::open(tmp_db.path()).await.unwrap();
        let prefs = ConvertPrefs {
            overwrite: true,
            ..Default::default()
        };

        // First export, added in place.
        let mut claimed = HashSet::new();
        let mut cancel = live_cancel();
        let out = convert_prepared(
            &db.engine,
            &row_for(&src, 1000),
            ConvertFormat::M4a,
            &prefs,
            &mut cancel,
            &|_| {},
            &mut claimed,
        )
        .await
        .unwrap();
        let Converted::File { path, refresh_row } = out else {
            panic!("expected a file");
        };
        assert!(refresh_row.is_none());
        let id = crate::library::ingest::probe_and_add(&db.engine, &path)
            .await
            .unwrap();

        // Second export onto the same name: allowed, and it names the
        // row to re-probe rather than refusing.
        let mut claimed = HashSet::new();
        let out = convert_prepared(
            &db.engine,
            &row_for(&src, 1000),
            ConvertFormat::M4a,
            &prefs,
            &mut cancel,
            &|_| {},
            &mut claimed,
        )
        .await
        .unwrap();
        assert!(matches!(out, Converted::File { refresh_row: Some(r), .. } if r == id));
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
    fn out_of_range_values_are_clamped_on_the_way_in() {
        // Whatever the webview or an old preferences blob sends, the
        // value that exists in the program is already in range.
        let wire = serde_json::json!({
            "flac": { "compression_level": 99, "sample_rate": 1, "bit_depth": 20 },
            "m4a": { "codec": "aac", "sample_rate": 192000, "bitrate_kbps": 4000 }
        });
        let prefs: ConvertPrefs = serde_json::from_value(wire).unwrap();
        assert_eq!(prefs.flac.compression_level, 12);
        assert_eq!(prefs.flac.sample_rate, Some(8_000));
        assert_eq!(prefs.flac.bit_depth, Some(24));
        assert_eq!(prefs.m4a.sample_rate, Some(96_000));
        assert_eq!(prefs.m4a.bitrate_kbps, 512);
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
