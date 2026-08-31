//! Writing playlists the device can actually read.
//!
//! Paths are relative to the playlist file, so a playlist stays valid
//! whether the device mounts its storage at `/`, at
//! `/storage/emulated/0`, or behind a drive letter — the single most
//! common reason exported playlists fail on Android.

use super::layout::sanitize_segment;
use super::transport::{Capabilities, DevicePath};

const EXTENSION: &str = ".m3u8";

/// One line of a playlist: a track already on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub device_path: DevicePath,
    pub duration_secs: u64,
    pub artist: String,
    pub title: String,
}

/// Render an extended M3U playlist.
///
/// UTF-8, no BOM, LF line endings: what Poweramp, USB Audio Player Pro,
/// Musicolet, Vinyl, VLC and most DAP firmware expect.
pub fn render_m3u8(entries: &[PlaylistEntry], playlist_dir: &DevicePath) -> String {
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        // A track that cannot be expressed relative to the playlist
        // (it *is* the playlist directory) has no sane line; skip it
        // rather than emit a path the player would mis-resolve.
        let Some(rel) = e.device_path.relative_to(playlist_dir) else {
            continue;
        };
        out.push_str(&format!(
            "#EXTINF:{},{} - {}\n{}\n",
            e.duration_secs, e.artist, e.title, rel
        ));
    }
    out
}

/// Build the on-device file name for a playlist.
///
/// Folder hierarchy is flattened — most Android players show a flat
/// playlist list, and nested directories of `.m3u8` files are commonly
/// missed by media scanners.
pub fn playlist_file_name(name: &str, ancestors: &[String], caps: &Capabilities) -> String {
    let mut parts: Vec<&str> = ancestors.iter().map(String::as_str).collect();
    parts.push(name);
    let joined = parts.join(" - ");
    let budget = caps.max_path_bytes.saturating_sub(EXTENSION.len());
    format!("{}{EXTENSION}", sanitize_segment(&joined, budget))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::transport::FilesystemKind;

    fn caps() -> Capabilities {
        Capabilities {
            playlist_objects: false,
            free_space: true,
            rename: true,
            max_path_bytes: 255,
            filesystem: FilesystemKind::ExFat,
        }
    }

    fn entry() -> PlaylistEntry {
        PlaylistEntry {
            device_path: DevicePath::new("/Music/Bonobo/Migration/01-02 Kerala.flac"),
            duration_secs: 243,
            artist: "Bonobo".into(),
            title: "Kerala".into(),
        }
    }

    #[test]
    fn renders_the_header_and_relative_paths() {
        let out = render_m3u8(&[entry()], &DevicePath::new("/Music/Playlists"));
        assert_eq!(
            out,
            "#EXTM3U\n\
             #EXTINF:243,Bonobo - Kerala\n\
             ../Bonobo/Migration/01-02 Kerala.flac\n"
        );
    }

    #[test]
    fn an_empty_playlist_still_has_a_header() {
        assert_eq!(
            render_m3u8(&[], &DevicePath::new("/Music/Playlists")),
            "#EXTM3U\n"
        );
    }

    #[test]
    fn output_has_no_bom_and_uses_lf() {
        let out = render_m3u8(&[entry()], &DevicePath::new("/Music/Playlists"));
        assert!(!out.starts_with('\u{feff}'));
        assert!(!out.contains('\r'));
    }

    #[test]
    fn a_track_beside_the_playlist_needs_no_prefix() {
        let e = PlaylistEntry {
            device_path: DevicePath::new("/Music/Playlists/a.flac"),
            ..entry()
        };
        let out = render_m3u8(&[e], &DevicePath::new("/Music/Playlists"));
        assert!(out.ends_with("\na.flac\n"), "got {out:?}");
    }

    #[test]
    fn folder_ancestry_is_flattened_into_the_file_name() {
        assert_eq!(
            playlist_file_name(
                "Deep Cuts",
                &["Electronic".to_string(), "Downtempo".to_string()],
                &caps()
            ),
            "Electronic - Downtempo - Deep Cuts.m3u8"
        );
    }

    #[test]
    fn the_playlist_file_name_is_sanitised() {
        assert_eq!(
            playlist_file_name("AC/DC: Best?", &[], &caps()),
            "AC-DC Best.m3u8"
        );
    }

    #[test]
    fn the_extension_fits_inside_the_device_name_cap() {
        let mut caps = caps();
        caps.max_path_bytes = 12;
        let name = playlist_file_name("A very long playlist name", &[], &caps);
        assert!(name.len() <= 12, "{name:?} exceeds the cap");
        assert!(name.ends_with(EXTENSION));
    }
}
