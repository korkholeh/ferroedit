//! The opened directory and its derived state.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Resolves the CLI path argument into a workspace root.
    ///
    /// A file argument opens its *parent* as the workspace (SPEC §10): the file
    /// itself becomes a tab, and the explorer still has a directory to show.
    pub fn from_arg(path: Option<&Path>) -> io::Result<Self> {
        let raw = match path {
            Some(p) => p.to_path_buf(),
            None => std::env::current_dir()?,
        };
        let root = if raw.is_file() {
            match raw.parent() {
                // `Path::parent` of a bare file name is the *empty* path rather
                // than `None`, and an empty root canonicalises to nothing: it
                // is what made `ferroedit a.txt` open a workspace called `/`,
                // with no tree, no repository and nothing for the watcher to
                // watch.
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => PathBuf::from("."),
            }
        } else {
            raw
        };
        // Canonicalise so the title is stable however the path was typed, but
        // keep the raw path when it does not exist yet rather than failing.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Short display name for the explorer header.
    pub fn name(&self) -> &str {
        self.root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ferroedit a.txt` from inside the directory: the parent of a bare file
    /// name is the empty path, and the workspace is the directory the editor
    /// was started in.
    #[test]
    fn a_bare_file_name_opens_the_directory_it_is_in() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let previous = std::env::current_dir().unwrap();
        // `set_current_dir` is process-wide, so this test may not assert
        // anything between the two calls that another test could observe.
        std::env::set_current_dir(dir.path()).unwrap();
        let workspace = Workspace::from_arg(Some(Path::new("main.rs")));
        std::env::set_current_dir(previous).unwrap();

        let root = workspace.unwrap().root().to_path_buf();
        assert_eq!(root, std::fs::canonicalize(dir.path()).unwrap());
        assert!(!root.as_os_str().is_empty());
    }

    #[test]
    fn a_file_argument_opens_its_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let workspace = Workspace::from_arg(Some(&file)).unwrap();
        assert_eq!(
            workspace.root(),
            std::fs::canonicalize(dir.path()).unwrap().as_path()
        );
    }

    #[test]
    fn a_directory_argument_is_the_root_itself() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_arg(Some(dir.path())).unwrap();
        assert_eq!(
            workspace.root(),
            std::fs::canonicalize(dir.path()).unwrap().as_path()
        );
    }

    #[test]
    fn a_missing_path_is_kept_verbatim() {
        let workspace = Workspace::from_arg(Some(Path::new("/no/such/dir"))).unwrap();
        assert_eq!(workspace.root(), Path::new("/no/such/dir"));
        assert_eq!(workspace.name(), "dir");
    }
}
