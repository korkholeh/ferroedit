//! Lazily expanded directory tree.
//!
//! A directory is read when it is first expanded and not before (SPEC §18), so
//! opening a repository with a hundred thousand files costs one `read_dir` of
//! the root. What the walk yields is filtered by `ignore`, which is what hides
//! git-ignored and hidden files (SPEC §19).
//!
//! The tree keeps a flattened `Vec<TreeRow>` of what is visible, rebuilt on
//! every structural change. Rendering, keyboard selection and mouse
//! hit-testing all index into that one vector, so a row on screen and the row a
//! command acts on cannot be different things.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
}

impl EntryKind {
    pub fn is_dir(self) -> bool {
        self == Self::Directory
    }
}

/// One node of the tree. `children` is `None` until the directory behind it has
/// been read — that `Option` *is* the laziness.
#[derive(Debug)]
struct Node {
    name: String,
    path: PathBuf,
    kind: EntryKind,
    expanded: bool,
    children: Option<Vec<Node>>,
}

/// One visible line of the explorer.
///
/// A row is a flat value rather than a borrow of a node, so `ui/` can read the
/// tree without holding it borrowed while `App` is passed around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: u16,
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub expanded: bool,
}

#[derive(Debug)]
pub struct FileTree {
    root: PathBuf,
    nodes: Vec<Node>,
    rows: Vec<TreeRow>,
    show_hidden: bool,
}

impl FileTree {
    /// Reads the root directory. Nothing below it is touched until it is
    /// expanded.
    pub fn new(root: &Path) -> Self {
        let mut tree = Self {
            root: root.to_path_buf(),
            nodes: Vec::new(),
            rows: Vec::new(),
            show_hidden: false,
        };
        tree.nodes = read_dir(root, tree.show_hidden);
        tree.rebuild_rows();
        tree
    }

    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, index: usize) -> Option<&TreeRow> {
        self.rows.get(index)
    }

    /// The row a path is on, if it is visible.
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|row| row.path == path)
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Toggles hidden and git-ignored files, and reloads what is already open.
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.show_hidden == show {
            return;
        }
        self.show_hidden = show;
        self.refresh();
    }

    /// Loads a directory's children if they have not been read yet, and shows
    /// them. A file is not a directory and expanding it does nothing.
    pub fn expand(&mut self, path: &Path) {
        let show_hidden = self.show_hidden;
        let Some(node) = node_at_path(&mut self.nodes, path) else {
            return;
        };
        if !node.kind.is_dir() {
            return;
        }
        if node.children.is_none() {
            node.children = Some(read_dir(&node.path, show_hidden));
        }
        node.expanded = true;
        self.rebuild_rows();
    }

    /// Hides a directory's children without forgetting them: re-expanding is
    /// then free, and a directory that has been looked at once keeps whatever
    /// the user has expanded inside it.
    pub fn collapse(&mut self, path: &Path) {
        if let Some(node) = node_at_path(&mut self.nodes, path) {
            if !node.kind.is_dir() {
                return;
            }
            node.expanded = false;
        }
        self.rebuild_rows();
    }

    pub fn toggle(&mut self, path: &Path) {
        let expanded = node_at_path(&mut self.nodes, path).is_some_and(|node| node.expanded);
        if expanded {
            self.collapse(path);
        } else {
            self.expand(path);
        }
    }

    /// Re-reads everything that is currently open, keeping the expanded
    /// directories expanded.
    ///
    /// This is what a file operation and `F5` both go through: rather than
    /// patching the tree at the point of the change — and getting the sort
    /// order, the ignore rules and the parent's laziness right by hand — the
    /// open directories are simply read again.
    pub fn refresh(&mut self) {
        let mut expanded = Vec::new();
        collect_expanded(&self.nodes, &mut expanded);
        self.nodes = read_dir(&self.root, self.show_hidden);
        // Shortest first, so a directory is expanded before the ones inside it.
        expanded.sort_by_key(|path| path.components().count());
        for path in expanded {
            self.expand(&path);
        }
        self.rebuild_rows();
    }

    /// Expands whatever it takes to make `path` visible, and returns its row.
    ///
    /// `None` for a path outside the workspace, or one that the ignore rules
    /// hide — a file created inside `target/` is really not in the tree, and
    /// saying so is better than selecting the wrong row.
    pub fn reveal(&mut self, path: &Path) -> Option<usize> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let mut current = self.root.clone();
        // Every component but the last is a directory that has to be open for
        // the last one to be on screen.
        let components: Vec<_> = relative.components().collect();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            current = current.join(component);
            self.expand(&current);
        }
        self.index_of(path)
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len().max(16));
        push_rows(&self.nodes, 0, &mut rows);
        self.rows = rows;
    }
}

fn push_rows(nodes: &[Node], depth: u16, rows: &mut Vec<TreeRow>) {
    for node in nodes {
        rows.push(TreeRow {
            depth,
            name: node.name.clone(),
            path: node.path.clone(),
            kind: node.kind,
            expanded: node.expanded,
        });
        if node.expanded {
            if let Some(children) = node.children.as_deref() {
                push_rows(children, depth + 1, rows);
            }
        }
    }
}

fn collect_expanded(nodes: &[Node], out: &mut Vec<PathBuf>) {
    for node in nodes {
        if node.expanded {
            out.push(node.path.clone());
        }
        if let Some(children) = node.children.as_deref() {
            collect_expanded(children, out);
        }
    }
}

/// The node at a path, found by descending rather than by scanning every node.
///
/// Written as "find the index, then borrow it" because returning a `&mut` from
/// the middle of an `iter_mut` loop and then continuing to use the iterator is
/// what the borrow checker rejects.
fn node_at_path<'a>(nodes: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
    let index = nodes.iter().position(|node| path.starts_with(&node.path))?;
    let node = &mut nodes[index];
    if node.path == path {
        return Some(node);
    }
    node_at_path(node.children.as_deref_mut()?, path)
}

/// One directory's worth of entries, filtered and sorted.
///
/// `max_depth(1)` is the lazy part: the walker is built per directory and never
/// descends. `require_git(false)` makes a `.gitignore` count outside a
/// repository too, which is what a user who wrote one expects; `.git` itself is
/// filtered out by name so that showing hidden files does not put it back.
fn read_dir(dir: &Path, show_hidden: bool) -> Vec<Node> {
    let mut nodes: Vec<Node> = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(!show_hidden)
        .git_global(!show_hidden)
        .git_exclude(!show_hidden)
        .require_git(false)
        .parents(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        // The walker yields the directory itself first; only its children are
        // rows.
        .filter_map(|entry| match entry {
            Ok(entry) if entry.depth() > 0 => Some(entry),
            Ok(_) => None,
            Err(err) => {
                log::warn!(
                    "skipping an unreadable entry under {}: {err}",
                    dir.display()
                );
                None
            }
        })
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let kind = match entry.file_type() {
                Some(file_type) if file_type.is_dir() => EntryKind::Directory,
                Some(_) => EntryKind::File,
                // A file type the walker could not read is not something to
                // guess at.
                None => return None,
            };
            Some(Node {
                name,
                path: entry.into_path(),
                kind,
                expanded: false,
                children: None,
            })
        })
        .collect();

    // Directories first, then case-insensitively by name — the order every file
    // manager uses, and stable across platforms whose `read_dir` is not.
    nodes.sort_by(|a, b| {
        b.kind
            .is_dir()
            .cmp(&a.kind.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A small project: two directories, a couple of files, and a `.gitignore`
    /// that hides one of them.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("src/ui")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/ui/theme.rs"), "// theme").unwrap();
        fs::write(root.join("target/binary"), "").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.join(".gitignore"), "/target\n").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        dir
    }

    fn names(tree: &FileTree) -> Vec<String> {
        tree.rows().iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn the_root_lists_directories_first_and_hides_what_git_ignores() {
        let dir = project();
        let tree = FileTree::new(dir.path());
        assert_eq!(
            names(&tree),
            vec!["src", "Cargo.toml"],
            "target/ is ignored, .gitignore and .hidden are hidden, src comes first"
        );
    }

    #[test]
    fn a_directory_is_read_only_when_it_is_expanded() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        assert_eq!(tree.len(), 2, "nothing below the root has been read");

        tree.expand(&dir.path().join("src"));
        assert_eq!(names(&tree), vec!["src", "ui", "main.rs", "Cargo.toml"]);
        assert_eq!(tree.rows()[1].depth, 1, "and it is drawn one level in");

        tree.expand(&dir.path().join("src/ui"));
        assert_eq!(
            names(&tree),
            vec!["src", "ui", "theme.rs", "main.rs", "Cargo.toml"]
        );
        assert_eq!(tree.rows()[2].depth, 2);
    }

    #[test]
    fn collapsing_hides_the_children_without_forgetting_them() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        let src = dir.path().join("src");
        tree.expand(&src);
        tree.expand(&dir.path().join("src/ui"));
        tree.collapse(&src);
        assert_eq!(names(&tree), vec!["src", "Cargo.toml"]);

        tree.expand(&src);
        assert_eq!(
            names(&tree),
            vec!["src", "ui", "theme.rs", "main.rs", "Cargo.toml"],
            "the nested directory is still open"
        );
    }

    #[test]
    fn toggling_goes_both_ways() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        let src = dir.path().join("src");
        tree.toggle(&src);
        assert!(tree.rows()[0].expanded);
        tree.toggle(&src);
        assert!(!tree.rows()[0].expanded);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn a_file_cannot_be_expanded() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        tree.expand(&dir.path().join("Cargo.toml"));
        assert_eq!(tree.len(), 2);
        assert!(!tree.rows()[1].expanded);
    }

    #[test]
    fn refreshing_picks_up_a_new_file_and_keeps_the_tree_open() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        tree.expand(&dir.path().join("src"));
        fs::write(dir.path().join("src/added.rs"), "").unwrap();

        tree.refresh();
        assert_eq!(
            names(&tree),
            vec!["src", "ui", "added.rs", "main.rs", "Cargo.toml"],
            "src is still expanded and the new file is in it"
        );
    }

    #[test]
    fn revealing_a_path_expands_everything_above_it() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        let theme = dir.path().join("src/ui/theme.rs");

        let index = tree.reveal(&theme).expect("a row");
        assert_eq!(tree.rows()[index].name, "theme.rs");
        assert_eq!(tree.rows()[index].depth, 2);
    }

    #[test]
    fn revealing_something_outside_the_workspace_finds_nothing() {
        let dir = project();
        let mut tree = FileTree::new(dir.path());
        assert_eq!(tree.reveal(Path::new("/etc/hosts")), None);
        assert_eq!(
            tree.reveal(&dir.path().join("target/binary")),
            None,
            "and an ignored file is not in the tree either"
        );
    }

    #[test]
    fn showing_hidden_files_brings_back_the_ignored_ones_but_never_dot_git() {
        let dir = project();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let mut tree = FileTree::new(dir.path());
        tree.set_show_hidden(true);

        let visible = names(&tree);
        assert!(visible.contains(&"target".to_string()));
        assert!(visible.contains(&".hidden".to_string()));
        assert!(visible.contains(&".gitignore".to_string()));
        assert!(
            !visible.contains(&".git".to_string()),
            "the repository's own directory is never a row"
        );

        tree.set_show_hidden(false);
        assert_eq!(names(&tree), vec!["src", "Cargo.toml"]);
    }

    #[test]
    fn an_empty_directory_has_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        let tree = FileTree::new(dir.path());
        assert_eq!(tree.len(), 0);
        assert!(tree.rows().is_empty());
    }

    #[test]
    fn a_root_that_does_not_exist_is_an_empty_tree_rather_than_a_panic() {
        let tree = FileTree::new(Path::new("/no/such/directory"));
        assert_eq!(tree.len(), 0);
    }
}
