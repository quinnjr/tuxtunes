//! CLI surface for the desktop (`tuxtunes`) binary.
//!
//! `--help` and `--version` are handled by clap before any GUI code
//! runs. Positional file/URL arguments are accepted but ignored: the
//! desktop entry runs `tuxtunes %U`, so opening media with TuxTunes
//! passes paths on every such launch. Rejecting them would break
//! file-association opens; they are reserved for future open-with
//! handling instead.
//!
//! Deliberately strict: unknown flags error (exit 2) instead of
//! launching the GUI, and leading-hyphen names are treated as flags,
//! so such a file must be passed after `--`.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "tuxtunes", version, about = "Music library manager and player")]
pub struct GuiArgs {
    /// Files or URLs to open (currently ignored; reserved for future use).
    pub files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn version_flag_reports_package_version_with_zero_exit() {
        let err = GuiArgs::try_parse_from(["tuxtunes", "--version"])
            .expect_err("--version must exit via clap, not launch the GUI");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert_eq!(err.exit_code(), 0);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn short_version_flag_matches_long() {
        let err =
            GuiArgs::try_parse_from(["tuxtunes", "-V"]).expect_err("-V must behave like --version");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn help_flag_renders_usage() {
        let err = GuiArgs::try_parse_from(["tuxtunes", "--help"])
            .expect_err("--help must exit via clap, not launch the GUI");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        assert!(err.to_string().contains("Music library manager and player"));
    }

    #[test]
    fn short_help_flag_matches_long() {
        let err =
            GuiArgs::try_parse_from(["tuxtunes", "-h"]).expect_err("-h must behave like --help");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    }

    #[test]
    fn bare_invocation_parses_to_empty_files() {
        // The common launch: app menu / desktop entry with no %U expansion.
        let args =
            GuiArgs::try_parse_from(["tuxtunes"]).expect("bare launch must parse with no files");
        assert!(args.files.is_empty());
    }

    #[test]
    fn positional_files_keep_their_values() {
        // The desktop entry runs `tuxtunes %U`: file-association opens
        // must not be rejected as unknown arguments.
        let args =
            GuiArgs::try_parse_from(["tuxtunes", "/music/song.flac", "https://example.com/stream"])
                .expect("positional files must parse");
        assert_eq!(
            args.files,
            vec![
                PathBuf::from("/music/song.flac"),
                PathBuf::from("https://example.com/stream"),
            ]
        );
    }

    #[test]
    fn unknown_flag_rejected_with_exit_2() {
        let err =
            GuiArgs::try_parse_from(["tuxtunes", "--bogus"]).expect_err("--bogus must not launch");
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn dashdash_treats_version_as_file() {
        // Standard `--` semantics: after it, even flag-like names are
        // positionals. A file literally named `--version` opens (and is
        // ignored); the version is intentionally not printed here.
        let args = GuiArgs::try_parse_from(["tuxtunes", "--", "--version"])
            .expect("name after `--` is a file, not a flag");
        assert_eq!(args.files, vec![PathBuf::from("--version")]);
    }

    #[test]
    fn version_with_extra_positional_still_reports_version() {
        // `--version` short-circuits; trailing files neither satisfy
        // nor break it.
        let err = GuiArgs::try_parse_from(["tuxtunes", "--version", "song.flac"])
            .expect_err("--version must win over extra args");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn empty_positional_is_rejected() {
        // An empty path is meaningless (`%U` expands to nothing, not to
        // `""`), so clap's PathBuf parser rejects it visibly instead of
        // silently launching.
        let err = GuiArgs::try_parse_from(["tuxtunes", ""])
            .expect_err("empty positional must error, not launch");
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
    }

    #[test]
    #[cfg(unix)]
    fn invalid_utf8_positional_survives_lossless() {
        // Pins the PathBuf (not String) value type: non-UTF-8 filenames
        // from USB sticks must keep opening the app, byte-identical.
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![0xFF, 0xFE, b'a']);
        let args = GuiArgs::try_parse_from(vec![OsString::from("tuxtunes"), raw.clone()])
            .expect("non-UTF8 file must parse via PathBuf");
        assert_eq!(args.files.len(), 1);
        assert_eq!(args.files[0].as_os_str(), &raw);
    }
}
