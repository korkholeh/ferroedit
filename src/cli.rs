//! CLI argument parsing (`ferroedit .`, `ferroedit +42 file.rs`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "ferroedit", version, about, long_about = None)]
pub struct Cli {
    /// Files to open, or one directory to open as the workspace. Defaults to
    /// the current directory.
    ///
    /// More than one file is what makes the tab bar reachable from the command
    /// line; until the explorer lands there is no other way to open a second
    /// tab.
    pub paths: Vec<PathBuf>,

    /// Line to place the cursor on, taken from a leading `+42` argument.
    #[arg(skip)]
    pub line: Option<usize>,

    /// Panic right after terminal setup, to verify the restore path by hand.
    #[arg(long, hide = true)]
    pub panic_test: bool,

    /// Write `docs/SHORTCUTS.md` to stdout and exit.
    #[arg(long, hide = true)]
    pub dump_shortcuts: bool,
}

impl Cli {
    /// Parses the real process arguments.
    ///
    /// `+42` is not expressible in clap's grammar (it is neither a flag nor a
    /// value), so it is stripped out before clap ever sees the argument list.
    pub fn parse_args() -> Self {
        Self::parse_argv(std::env::args_os())
    }

    /// The argument the workspace is taken from: the first path, if any.
    pub fn workspace_arg(&self) -> Option<&Path> {
        self.paths.first().map(PathBuf::as_path)
    }

    fn parse_argv(argv: impl IntoIterator<Item = OsString>) -> Self {
        let mut line = None;
        let mut kept = Vec::new();
        for arg in argv {
            match arg.to_str().and_then(parse_line_arg) {
                // A second `+N` falls through to clap, which reports it as an
                // unexpected argument rather than silently ignoring it.
                Some(n) if line.is_none() => line = Some(n),
                _ => kept.push(arg),
            }
        }
        let mut cli = Cli::parse_from(kept);
        cli.line = line;
        cli
    }
}

fn parse_line_arg(arg: &str) -> Option<usize> {
    let digits = arg.strip_prefix('+')?;
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn plain_path_argument() {
        let cli = Cli::parse_argv(argv(&["ferroedit", "src/main.rs"]));
        assert_eq!(cli.paths, vec![PathBuf::from("src/main.rs")]);
        assert_eq!(cli.workspace_arg(), Some(Path::new("src/main.rs")));
        assert_eq!(cli.line, None);
    }

    #[test]
    fn several_files_open_several_tabs() {
        let cli = Cli::parse_argv(argv(&["ferroedit", "a.rs", "b.rs", "c.rs"]));
        assert_eq!(
            cli.paths,
            vec![
                PathBuf::from("a.rs"),
                PathBuf::from("b.rs"),
                PathBuf::from("c.rs")
            ]
        );
        // The workspace comes from the first one, as it does with a single file.
        assert_eq!(cli.workspace_arg(), Some(Path::new("a.rs")));
    }

    #[test]
    fn leading_line_argument_is_extracted() {
        let cli = Cli::parse_argv(argv(&["ferroedit", "+42", "src/main.rs"]));
        assert_eq!(cli.line, Some(42));
        assert_eq!(cli.paths, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn no_arguments_means_current_directory() {
        let cli = Cli::parse_argv(argv(&["ferroedit"]));
        assert!(cli.paths.is_empty());
        assert_eq!(cli.workspace_arg(), None);
    }

    #[test]
    fn the_shortcuts_dump_is_a_flag_and_not_a_path() {
        let cli = Cli::parse_argv(argv(&["ferroedit", "--dump-shortcuts"]));
        assert!(cli.dump_shortcuts);
        assert!(cli.paths.is_empty(), "it opens nothing");
        assert!(!Cli::parse_argv(argv(&["ferroedit"])).dump_shortcuts);
    }

    #[test]
    fn plus_without_digits_is_not_a_line() {
        assert_eq!(parse_line_arg("+"), None);
        assert_eq!(parse_line_arg("+abc"), None);
        assert_eq!(parse_line_arg("+7"), Some(7));
    }
}
