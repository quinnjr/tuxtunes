//! Audio fixtures shared by the `library` tests. `#[cfg(test)]` only.

use crate::db::tracks::MetadataEdit;
use std::path::Path;

/// A minimal WAV with an even-length data chunk. RIFF requires odd
/// chunks to be padded, and a one-byte data chunk has no pad, so a tag
/// appended after it is not found on re-read — fine for the untagged
/// tests, not for one that writes metadata.
pub(crate) fn write_taggable_wav(path: &Path) {
    let bytes: &[u8] = &[
        b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
        0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
        b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
    ];
    std::fs::write(path, bytes).unwrap();
}

/// A [`write_taggable_wav`] file tagged with track 1, disc 2.
pub(crate) fn tagged_wav(path: &Path) {
    write_taggable_wav(path);
    crate::fs::tags::write_metadata(
        path,
        &MetadataEdit {
            title: "Tagged",
            artist: None,
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: Some(1),
            disc_number: Some(2),
        },
    )
    .unwrap();
}

/// A minimal FLAC: the "fLaC" marker, a STREAMINFO block, and one
/// VORBIS_COMMENT block, with no audio frames. Lofty reads the
/// properties and the comments from it just fine, which is all the
/// probe needs.
pub(crate) fn write_minimal_flac(path: &Path, comments: &[&str]) {
    fn be24(n: usize) -> [u8; 3] {
        [(n >> 16) as u8, (n >> 8) as u8, n as u8]
    }

    // STREAMINFO: 4096-sample blocks, 44100 Hz, 2 channels, 16-bit,
    // 44100 samples.
    let mut streaminfo = Vec::with_capacity(34);
    streaminfo.extend_from_slice(&4096u16.to_be_bytes());
    streaminfo.extend_from_slice(&4096u16.to_be_bytes());
    streaminfo.extend_from_slice(&[0, 0, 0]); // min frame size
    streaminfo.extend_from_slice(&[0, 0, 0]); // max frame size
    let packed: u64 = (44100u64 << 44) | (1u64 << 41) | (15u64 << 36) | 44100u64;
    streaminfo.extend_from_slice(&packed.to_be_bytes());
    streaminfo.extend_from_slice(&[0u8; 16]); // md5

    let vendor = b"tuxtunes-test";
    let mut vc = Vec::new();
    vc.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    vc.extend_from_slice(vendor);
    vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in comments {
        vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
        vc.extend_from_slice(c.as_bytes());
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"fLaC");
    out.push(0x00); // metadata header: not last, type 0 (STREAMINFO)
    out.extend_from_slice(&be24(streaminfo.len()));
    out.extend_from_slice(&streaminfo);
    out.push(0x84); // metadata header: last, type 4 (Vorbis comment)
    out.extend_from_slice(&be24(vc.len()));
    out.extend_from_slice(&vc);
    std::fs::write(path, out).unwrap();
}
