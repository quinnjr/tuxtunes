//! Render an `organize_scheme` template for a track row, with
//! path-component sanitization and collision-suffix resolution.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TrackFields<'a> {
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album_artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub track_number: Option<u16>,
    pub track_count: Option<u16>,
    pub disc_number: Option<u16>,
    pub disc_count: Option<u16>,
    pub year: Option<u16>,
    pub ext: &'a str,
    /// Fallback filename stem used if `title` is empty (e.g. from the
    /// source's basename on copy-from-folder paths).
    pub fallback_stem: &'a str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathRenderError {
    #[error("unknown token: {0}")]
    UnknownToken(String),
    #[error("malformed template: {0}")]
    Malformed(String),
    #[error("missing extension")]
    MissingExt,
}

/// Render `template` against `t`, returning a managed-root-relative
/// `PathBuf`. Missing Optionals fall back per the spec:
///   album_artist → artist → "Unknown Artist"
///   artist       → "Unknown Artist"
///   album        → "Unknown Album"
///   title        → fallback_stem → "Unknown Title"
///   track/disc   → 0
///   year / genre → empty string
///   ext          → required (errors if absent at call site)
pub fn render(template: &str, t: &TrackFields<'_>) -> Result<PathBuf, PathRenderError> {
    if t.ext.is_empty() {
        return Err(PathRenderError::MissingExt);
    }
    let expanded = expand_tokens(template, t)?;
    let components: Vec<String> = expanded
        .split('/')
        .map(sanitize_component)
        .filter(|s| !s.is_empty())
        .collect();
    if components.is_empty() {
        return Err(PathRenderError::Malformed("empty path".into()));
    }
    Ok(components.iter().collect::<PathBuf>())
}

/// If `candidate` (managed-root-absolute) exists, append ` (2)`, ` (3)`, …
/// to the file stem until an unused name is found, capped at 999.
pub fn resolve_collision(candidate: &Path) -> PathBuf {
    if !candidate.exists() {
        return candidate.to_path_buf();
    }
    let parent = candidate.parent().unwrap_or_else(|| Path::new(""));
    let stem = candidate
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = candidate.extension().and_then(|s| s.to_str()).unwrap_or("");
    for n in 2u32..=999 {
        let name = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let cand = parent.join(&name);
        if !cand.exists() {
            return cand;
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = if ext.is_empty() {
        format!("{stem} ({ts})")
    } else {
        format!("{stem} ({ts}).{ext}")
    };
    parent.join(name)
}

/// Sanitize a single path component: replace `/` with `-`, strip control
/// chars, collapse whitespace runs, trim trailing dots/spaces. Preserves
/// Unicode.
fn sanitize_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for ch in raw.chars() {
        let replaced = match ch {
            '/' => '-',
            c if c.is_control() => continue,
            c => c,
        };
        let is_space = replaced.is_whitespace();
        if is_space && last_was_space {
            continue;
        }
        out.push(replaced);
        last_was_space = is_space;
    }
    out.trim_start_matches('-')
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

fn expand_tokens(template: &str, t: &TrackFields<'_>) -> Result<String, PathRenderError> {
    let mut out = String::with_capacity(template.len() * 2);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        let mut token = String::new();
        let mut closed = false;
        for c in chars.by_ref() {
            if c == '}' {
                closed = true;
                break;
            }
            if c == '{' {
                return Err(PathRenderError::Malformed(format!(
                    "nested '{{' in token '{{{token}'"
                )));
            }
            token.push(c);
        }
        if !closed {
            return Err(PathRenderError::Malformed(format!(
                "unterminated token starting with '{{{token}'"
            )));
        }
        let rendered = render_token(&token, t)?;
        out.push_str(&rendered);
    }
    Ok(out)
}

fn render_token(token: &str, t: &TrackFields<'_>) -> Result<String, PathRenderError> {
    let (name, width) = match token.split_once(':') {
        Some((n, w)) => {
            let w: usize = w
                .parse()
                .map_err(|_| PathRenderError::Malformed(format!("bad width: {token}")))?;
            (n, Some(w))
        }
        None => (token, None),
    };
    let s = match name {
        "album_artist" => {
            sanitize_component(t.album_artist.or(t.artist).unwrap_or("Unknown Artist"))
        }
        "artist" => sanitize_component(t.artist.unwrap_or("Unknown Artist")),
        "album" => sanitize_component(t.album.unwrap_or("Unknown Album")),
        "title" => {
            let raw = if t.title.is_empty() {
                if t.fallback_stem.is_empty() {
                    "Unknown Title"
                } else {
                    t.fallback_stem
                }
            } else {
                t.title
            };
            sanitize_component(raw)
        }
        "genre" => sanitize_component(t.genre.unwrap_or("")),
        "year" => t.year.map(|y| y.to_string()).unwrap_or_default(),
        "track" => format_int(u64::from(t.track_number.unwrap_or(0)), width),
        "disc" => {
            // Omit disc when disc_count <= 1 AND no explicit disc number,
            // to keep single-CD albums tidy.
            let multi = t.disc_count.unwrap_or(1) > 1 || t.disc_number.unwrap_or(0) > 1;
            if multi {
                format_int(u64::from(t.disc_number.unwrap_or(0)), width)
            } else {
                String::new()
            }
        }
        "ext" => t.ext.to_string(),
        other => return Err(PathRenderError::UnknownToken(other.to_string())),
    };
    Ok(s)
}

fn format_int(v: u64, width: Option<usize>) -> String {
    match width {
        Some(w) => format!("{v:0width$}", width = w),
        None => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(title: &'static str) -> TrackFields<'static> {
        TrackFields {
            title,
            artist: Some("The Beatles"),
            album_artist: Some("The Beatles"),
            album: Some("Abbey Road"),
            genre: Some("Rock"),
            track_number: Some(3),
            track_count: Some(17),
            disc_number: Some(1),
            disc_count: Some(1),
            year: Some(1969),
            ext: "flac",
            fallback_stem: "01 Something",
        }
    }

    #[test]
    fn renders_default_template() {
        let p = render(
            "{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}",
            &t("Something"),
        )
        .unwrap();
        assert_eq!(
            p.to_str().unwrap(),
            "The Beatles/Abbey Road/03 - Something.flac"
        );
    }

    #[test]
    fn multi_disc_includes_disc_token() {
        let mut tf = t("Something");
        tf.disc_count = Some(2);
        tf.disc_number = Some(2);
        let p = render(
            "{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}",
            &tf,
        )
        .unwrap();
        assert_eq!(
            p.to_str().unwrap(),
            "The Beatles/Abbey Road/02-03 - Something.flac"
        );
    }

    #[test]
    fn slash_in_field_becomes_dash() {
        let mut tf = t("A/B");
        tf.album = Some("One/Two");
        let p = render("{album}/{title}.{ext}", &tf).unwrap();
        assert_eq!(p.to_str().unwrap(), "One-Two/A-B.flac");
    }

    #[test]
    fn control_chars_stripped() {
        let mut tf = t("hello\u{7}world");
        tf.album_artist = Some("Artist\u{1f}ish");
        let p = render("{album_artist}/{title}.{ext}", &tf).unwrap();
        assert_eq!(p.to_str().unwrap(), "Artistish/helloworld.flac");
    }

    #[test]
    fn trailing_dots_and_spaces_trimmed() {
        let mut tf = t("Song.  ");
        tf.album = Some("...Album. ");
        let p = render("{album}/{title}.{ext}", &tf).unwrap();
        assert_eq!(p.to_str().unwrap(), "Album/Song.flac");
    }

    #[test]
    fn missing_album_artist_falls_back_to_artist() {
        let mut tf = t("X");
        tf.album_artist = None;
        let p = render("{album_artist}/{title}.{ext}", &tf).unwrap();
        assert_eq!(p.to_str().unwrap(), "The Beatles/X.flac");
    }

    #[test]
    fn missing_everything_produces_unknown_placeholders() {
        let tf = TrackFields {
            title: "",
            artist: None,
            album_artist: None,
            album: None,
            genre: None,
            track_number: None,
            track_count: None,
            disc_number: None,
            disc_count: None,
            year: None,
            ext: "mp3",
            fallback_stem: "",
        };
        let p = render("{album_artist}/{album}/{title}.{ext}", &tf).unwrap();
        assert_eq!(
            p.to_str().unwrap(),
            "Unknown Artist/Unknown Album/Unknown Title.mp3"
        );
    }

    #[test]
    fn unknown_token_errors() {
        let err = render("{nope}.{ext}", &t("x")).unwrap_err();
        assert!(matches!(err, PathRenderError::UnknownToken(_)));
    }

    #[test]
    fn missing_ext_errors() {
        let mut tf = t("x");
        tf.ext = "";
        assert!(matches!(
            render("{title}.{ext}", &tf),
            Err(PathRenderError::MissingExt)
        ));
    }

    #[test]
    fn unterminated_token_errors() {
        let err = render("{album_artist/{title}.{ext}", &t("x")).unwrap_err();
        assert!(matches!(err, PathRenderError::Malformed(_)));
    }

    #[test]
    fn collision_picks_next_free_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("foo.flac");
        std::fs::write(&f, b"").unwrap();
        let next = resolve_collision(&f);
        assert_eq!(next, dir.path().join("foo (2).flac"));

        std::fs::write(dir.path().join("foo (2).flac"), b"").unwrap();
        let next = resolve_collision(&f);
        assert_eq!(next, dir.path().join("foo (3).flac"));
    }

    #[test]
    fn collision_noop_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("nope.flac");
        assert_eq!(resolve_collision(&f), f);
    }
}
