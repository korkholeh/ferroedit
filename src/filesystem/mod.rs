//! Lazy, gitignore-aware project tree and the file operations behind the
//! explorer (SPEC §20).
//!
//! Every operation here takes a *name* rather than a path and joins it itself,
//! so nothing typed into a dialog can escape the directory it was typed into.
//! Failures come back as errors and are shown to the user; nothing is logged
//! and swallowed.

pub mod tree;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("a name cannot be empty")]
    EmptyName,
    #[error("\"{0}\" is not a name — it contains a path separator")]
    NotAName(String),
    #[error("{0} already exists")]
    Exists(String),
}

impl FsError {
    /// Named by the entry rather than by its whole path: the user is looking at
    /// the directory it would go into, and the status bar is one line.
    fn exists(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or_else(|| path.display().to_string(), str::to_string);
        Self::Exists(name)
    }

    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// Creates an empty file and returns where it landed.
///
/// `create_new` is what makes the "does it already exist" check part of the
/// same syscall: between a check and a write, another process — or another
/// FerroEdit — could have created the file, and overwriting someone's work
/// because of that gap is not a bug worth risking for a tidier API.
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, FsError> {
    let path = target(parent, name)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(FsError::exists(&path)),
        Err(err) => Err(FsError::io(&path, err)),
    }
}

/// Creates one directory. Intermediate directories are not created: a name with
/// a separator in it is rejected before this point.
pub fn create_directory(parent: &Path, name: &str) -> Result<PathBuf, FsError> {
    let path = target(parent, name)?;
    match fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(FsError::exists(&path)),
        Err(err) => Err(FsError::io(&path, err)),
    }
}

/// Renames a file or directory inside its own parent.
///
/// The existence check is not atomic here — `fs::rename` overwrites its target
/// on Unix and there is no stable std API that refuses to — so it is done
/// explicitly. The race is real and small; silently replacing a file the user
/// can see in the tree is neither.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, FsError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let destination = target(parent, new_name)?;
    if destination == path {
        return Ok(destination);
    }
    if destination.exists() {
        return Err(FsError::exists(&destination));
    }
    fs::rename(path, &destination).map_err(|err| FsError::io(path, err))?;
    Ok(destination)
}

/// Deletes a file, or a directory and everything in it.
///
/// The recursive form is deliberate: a delete that refuses non-empty
/// directories would leave the user emptying one by hand from a file tree that
/// has no multi-select. The confirmation dialog is what makes it safe, and it
/// says which of the two is about to happen.
pub fn delete(path: &Path) -> Result<(), FsError> {
    let outcome = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    outcome.map_err(|err| FsError::io(path, err))
}

/// Validates a typed name and joins it onto its parent.
fn target(parent: &Path, name: &str) -> Result<PathBuf, FsError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(FsError::EmptyName);
    }
    // A name is one path component. Rejecting separators and `..` is what keeps
    // "New File" from writing outside the directory it was invoked in.
    if name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) || name == "." || name == ".."
    {
        return Err(FsError::NotAName(name.to_string()));
    }
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_is_created_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_file(dir.path(), "notes.md").unwrap();
        assert_eq!(path, dir.path().join("notes.md"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn creating_over_something_that_exists_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        create_file(dir.path(), "notes.md").unwrap();
        fs::write(dir.path().join("notes.md"), "work").unwrap();

        let err = create_file(dir.path(), "notes.md").unwrap_err();
        assert!(matches!(err, FsError::Exists(_)));
        assert_eq!(
            fs::read_to_string(dir.path().join("notes.md")).unwrap(),
            "work",
            "and the file that was there is untouched"
        );
    }

    #[test]
    fn a_name_is_one_component_and_never_a_path() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../escape.txt", "sub/file.txt", "..", "."] {
            let err = create_file(dir.path(), name).unwrap_err();
            assert!(
                matches!(err, FsError::NotAName(_)),
                "{name} should not be a name"
            );
        }
        assert!(matches!(
            create_file(dir.path(), "   ").unwrap_err(),
            FsError::EmptyName
        ));
    }

    #[test]
    fn a_directory_is_created_and_can_be_created_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_directory(dir.path(), "src").unwrap();
        assert!(path.is_dir());
        assert!(matches!(
            create_directory(dir.path(), "src").unwrap_err(),
            FsError::Exists(_)
        ));
    }

    #[test]
    fn renaming_moves_the_file_inside_its_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.txt");
        fs::write(&old, "text").unwrap();

        let new = rename(&old, "new.txt").unwrap();
        assert_eq!(new, dir.path().join("new.txt"));
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(&new).unwrap(), "text");
    }

    #[test]
    fn renaming_onto_an_existing_file_is_refused_rather_than_overwriting_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();

        let err = rename(&dir.path().join("a.txt"), "b.txt").unwrap_err();
        assert!(matches!(err, FsError::Exists(_)));
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "b",
            "b.txt is still b.txt"
        );
    }

    #[test]
    fn renaming_a_file_to_its_own_name_does_nothing_and_says_so_by_succeeding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "a").unwrap();
        assert_eq!(rename(&path, "a.txt").unwrap(), path);
        assert!(path.exists());
    }

    #[test]
    fn deleting_removes_a_file_and_a_whole_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "a").unwrap();
        delete(&file).unwrap();
        assert!(!file.exists());

        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inside.txt"), "x").unwrap();
        delete(&sub).unwrap();
        assert!(!sub.exists());
    }

    #[test]
    fn deleting_something_that_is_not_there_is_an_error_with_its_path_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let err = delete(&dir.path().join("ghost.txt")).unwrap_err();
        assert!(err.to_string().contains("ghost.txt"));
    }
}
