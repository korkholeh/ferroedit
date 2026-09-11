//! Settings: what the editor remembers between runs, and where it keeps it.
//!
//! The file is `~/.config/ferroedit/config.json` — JSON rather than the TOML
//! the rest of the project reads, and `.config` rather than whatever
//! `directories` calls a config directory on the platform, because the path is
//! the one thing about a config file a user has to be able to guess. It is the
//! same path on macOS and on Linux, and `$XDG_CONFIG_HOME` moves it when it is
//! set.
//!
//! Every failure here is non-fatal by design: a missing, unreadable or
//! malformed file leaves the defaults in place. An editor that refused to
//! start over its own preferences file would be an editor that cannot be used
//! to fix it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One of the built-in colour schemes (SPEC §43).
///
/// The name is what is written to the config file, so the serialised form is
/// spelled out rather than derived from the variant: renaming a variant must
/// not silently reset everybody's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
    /// The same layout of colours drawn from the sixteen ANSI ones, for a
    /// terminal whose 256-colour approximations are poor — or a user who has
    /// spent an afternoon on their own palette and wants the editor to use it.
    DarkSimple,
    LightSimple,
    /// Blue ground, yellow text, grey chrome and a green highlight: the
    /// DOS-era full-screen editor.
    ///
    /// The alias is the name it shipped under in 0.1.x, so a config written
    /// then still parses — and a file whose theme did not parse is a file
    /// whose word wrap is thrown away with it.
    #[serde(alias = "borland")]
    Retro,
}

impl ThemeKind {
    /// Every theme, in the order the picker lists them.
    ///
    /// It was the tests' alone while the View menu spelled the five out as
    /// five entries. The picker behind View → Theme… is built from this list
    /// instead (ADR-079), so a theme added here is reachable by that fact —
    /// and the test that used to check the menu now checks the picker.
    pub const ALL: [ThemeKind; 5] = [
        Self::Dark,
        Self::Light,
        Self::DarkSimple,
        Self::LightSimple,
        Self::Retro,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::DarkSimple => "Dark Simple",
            Self::LightSimple => "Light Simple",
            Self::Retro => "Retro",
        }
    }
}

/// Everything the editor persists. `#[serde(default)]` is what lets the next
/// field be added without invalidating the files already written — and what
/// let word wrap join the theme here.
///
/// `Default` is written out rather than derived from Phase 15 on: the update
/// check is the first setting whose default is not the zero value, and a
/// derived `false` there would have shipped the feature switched off for
/// everybody who has a config file already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    pub theme: ThemeKind,
    /// Whether lines longer than the pane are broken onto the next row rather
    /// than run off the right edge (SPEC §58, ADR-057).
    ///
    /// Off by default: code is written to a column limit and read with the
    /// indentation carrying the structure, and a wrapped pane is the answer
    /// for the file that was not — which is a choice the user makes per
    /// session and the editor then remembers.
    pub word_wrap: bool,
    /// Whether the editor asks GitHub at start-up whether there is a newer
    /// release (SPEC §66, ADR-065).
    ///
    /// On by default, because a user who installed a single binary with a
    /// script has no package manager to tell them — and off in one menu entry,
    /// because a request to a third party that the user did not make is
    /// something they are entitled to stop. Nothing about the request
    /// identifies them beyond what any HTTP client sends.
    pub check_for_updates: bool,
    /// When the last check answered, in seconds since the epoch, so a start-up
    /// check happens at most once a day rather than once a launch.
    ///
    /// Bookkeeping rather than a preference — it is in this file because this
    /// is the file the editor already writes, and a second one for a single
    /// number would be a second path to explain.
    pub last_update_check: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeKind::default(),
            word_wrap: false,
            check_for_updates: true,
            last_update_check: None,
        }
    }
}

impl Settings {
    /// `~/.config/ferroedit/config.json`, or `None` when there is no home to
    /// hang it off — which is what a test environment and a daemon both look
    /// like.
    pub fn path() -> Option<PathBuf> {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
        };
        Some(base.join("ferroedit").join("config.json"))
    }

    /// Reads the file, falling back to the defaults for every reason it might
    /// not be readable — including a file whose JSON no longer parses.
    pub fn load() -> Self {
        match Self::path() {
            Some(path) => Self::load_from(&path),
            None => Self::default(),
        }
    }

    /// Writes the file, creating `~/.config/ferroedit` if it is not there.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
        self.save_to(&path)
    }

    /// The two halves above, with the path handed in.
    ///
    /// Split out so the round trip can be tested against a temporary file:
    /// the alternative is a test that reaches into the environment, and a test
    /// that writes to the config of whoever is running it is a test nobody
    /// runs twice.
    pub fn load_from(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                log::warn!("could not read {}: {err}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                log::warn!("ignoring {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Pretty-printed with a trailing newline: it is a file a user is expected
    /// to open, and one long line is not.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_theme_is_the_dark_one() {
        assert_eq!(Settings::default().theme, ThemeKind::Dark);
    }

    #[test]
    fn lines_do_not_wrap_until_the_user_says_so() {
        assert!(!Settings::default().word_wrap);
    }

    #[test]
    fn the_wrap_setting_round_trips_through_the_file_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let settings = Settings {
            theme: ThemeKind::Dark,
            word_wrap: true,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("word-wrap"), "{text}");
        assert_eq!(Settings::load_from(&path), settings);
    }

    /// A file written by 0.1.x has no `check-for-updates` key in it, and the
    /// derived `Default` would have read that absence as "switched off".
    #[test]
    fn the_update_check_is_on_until_the_user_turns_it_off() {
        assert!(Settings::default().check_for_updates);
        assert_eq!(Settings::default().last_update_check, None);

        let old: Settings = serde_json::from_str(r#"{"theme": "light"}"#).unwrap();
        assert!(old.check_for_updates, "an old config keeps the new default");
        assert_eq!(old.theme, ThemeKind::Light);
    }

    #[test]
    fn the_update_check_round_trips_through_the_file_format() {
        let settings = Settings {
            check_for_updates: false,
            last_update_check: Some(1_700_000_000),
            ..Settings::default()
        };
        let text = serde_json::to_string(&settings).unwrap();
        assert!(text.contains("check-for-updates"), "{text}");
        assert!(text.contains("last-update-check"), "{text}");
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn a_theme_round_trips_through_the_file_format() {
        let settings = Settings {
            theme: ThemeKind::LightSimple,
            ..Settings::default()
        };
        let text = serde_json::to_string(&settings).unwrap();
        assert!(text.contains("light-simple"), "{text}");
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn an_unknown_key_and_a_missing_one_both_leave_the_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"fontSize": 12}"#).unwrap();
        assert_eq!(settings, Settings::default());
    }

    /// The path is the one thing about a config file that has to be
    /// predictable, so it is asserted rather than assumed.
    #[test]
    fn the_config_lives_under_dot_config() {
        let path = Settings::path().expect("the test runner has a home");
        assert!(
            path.ends_with("ferroedit/config.json"),
            "{}",
            path.display()
        );
    }

    /// The round trip through a real file, which is the thing that has to
    /// work: the theme chosen in one run is the theme the next one starts in.
    #[test]
    fn a_theme_survives_a_write_and_a_read() {
        let dir = tempfile::tempdir().unwrap();
        // Under a directory that does not exist yet, like the first run's.
        let path = dir.path().join("ferroedit").join("config.json");
        let settings = Settings {
            theme: ThemeKind::Retro,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), settings);
        assert!(
            fs::read_to_string(&path).unwrap().ends_with("\n"),
            "the file ends in a newline"
        );
    }

    /// Nothing here is worth refusing to start over.
    #[test]
    fn a_missing_or_broken_file_leaves_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nothing.json");
        assert_eq!(Settings::load_from(&missing), Settings::default());

        let broken = dir.path().join("broken.json");
        fs::write(&broken, "{ this is not json").unwrap();
        assert_eq!(Settings::load_from(&broken), Settings::default());

        let unknown = dir.path().join("unknown.json");
        fs::write(&unknown, r#"{"theme": "solarized"}"#).unwrap();
        assert_eq!(Settings::load_from(&unknown), Settings::default());
    }

    #[test]
    fn every_theme_has_a_label() {
        for kind in ThemeKind::ALL {
            assert!(!kind.label().is_empty());
        }
    }
}
