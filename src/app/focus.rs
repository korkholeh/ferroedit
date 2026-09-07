//! Which pane currently receives input.

/// Keyboard routing target (SPEC §26).
///
/// `Dialog` arrived in Phase 5 with its first producer, the close-with-confirm
/// prompt. It is modal: while it holds focus nothing else resolves a key, not
/// even a global binding, which is enforced in `event::keyboard::resolve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    Editor,
    Explorer,
    GitPanel,
    Menu,
    Dialog,
    /// The read-only diff viewer (SPEC §36). Like the find bar it is entered
    /// and left explicitly and never cycled into — and unlike it, it covers
    /// the editor pane, so it closes as soon as another pane takes focus.
    Diff,
    /// The help screen (SPEC §6). A pager over the keymap, drawn over the whole
    /// body, and closed by the same rule as the diff viewer (ADR-038).
    Help,
    /// The find/replace bar. Reached by `Ctrl+F` and left by `Esc`, never by
    /// the cycle key: it is a transient tool, not one of the panes.
    Search,
}

impl FocusTarget {
    /// Panes reachable by the cycle key. The menu and the search bar are
    /// entered explicitly and left explicitly, and a dialog is modal, so none
    /// of the three is ever cycled into.
    const CYCLE: [FocusTarget; 3] = [Self::Editor, Self::Explorer, Self::GitPanel];

    pub fn next(self) -> Self {
        match Self::CYCLE.iter().position(|f| *f == self) {
            Some(i) => Self::CYCLE[(i + 1) % Self::CYCLE.len()],
            // Leaving the menu by cycling lands back on the editor.
            None => Self::Editor,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Editor => "Editor",
            Self::Explorer => "Explorer",
            Self::GitPanel => "Git",
            Self::Menu => "Menu",
            Self::Dialog => "Dialog",
            Self::Search => "Search",
            Self::Diff => "Diff",
            Self::Help => "Help",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_visits_every_pane_and_wraps() {
        let mut seen = vec![FocusTarget::Editor];
        for _ in 0..3 {
            seen.push(seen.last().unwrap().next());
        }
        assert_eq!(
            seen,
            vec![
                FocusTarget::Editor,
                FocusTarget::Explorer,
                FocusTarget::GitPanel,
                FocusTarget::Editor
            ]
        );
    }

    #[test]
    fn cycling_out_of_the_menu_returns_to_the_editor() {
        assert_eq!(FocusTarget::Menu.next(), FocusTarget::Editor);
        assert_eq!(FocusTarget::Dialog.next(), FocusTarget::Editor);
        assert_eq!(FocusTarget::Search.next(), FocusTarget::Editor);
        assert_eq!(FocusTarget::Diff.next(), FocusTarget::Editor);
        assert_eq!(FocusTarget::Help.next(), FocusTarget::Editor);
    }
}
