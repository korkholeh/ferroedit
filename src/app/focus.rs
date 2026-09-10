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
    /// A diff tab (SPEC §36, ADR-053). It stands where `Editor` does — the
    /// same pane, showing a diff instead of a document — so the cycle key
    /// treats the two as one stop, and `App::normalize_focus` picks whichever
    /// of them matches the tab in front.
    Diff,
    /// The help screen (SPEC §6). A pager over the keymap, drawn over the whole
    /// body, and closed by the same rule as the diff viewer (ADR-038).
    Help,
    /// The find/replace bar. Reached by `Ctrl+F` and left by `Esc`, never by
    /// the cycle key: it is a transient tool, not one of the panes.
    Search,
    /// A log tab (ADR-068). It stands where `Editor` and `Diff` do — the same
    /// pane, showing a history instead of a document — so the cycle key treats
    /// the three as one stop.
    Log,
    /// An image tab (ADR-078). It stands where `Editor`, `Diff` and `Log` do —
    /// the same pane, showing a picture instead of a document — so the cycle
    /// key treats them all as one stop.
    Image,
    /// The log's search field, while it has the caret.
    ///
    /// A focus of its own rather than a flag, for the reason the find bar is
    /// one: the pane's keys are a pager's — `d`, `Enter`, `Home` — and a field
    /// that shared them could not type a `d`. `resolve` picks a table from the
    /// focus, so the two meanings can never both match (SPEC §22).
    LogSearch,
}

impl FocusTarget {
    /// Panes reachable by the cycle key. The menu and the two search fields are
    /// entered explicitly and left explicitly, and a dialog is modal, so none
    /// of them is ever cycled into.
    const CYCLE: [FocusTarget; 3] = [Self::Editor, Self::Explorer, Self::GitPanel];

    pub fn next(self) -> Self {
        // A diff tab takes the editor's place in the cycle, because it *is*
        // what the editor pane is showing (SPEC §36). Without this the cycle
        // key could not leave a diff: `App::normalize_focus` would put focus
        // straight back on it.
        let from = if matches!(self, Self::Diff | Self::Log | Self::LogSearch | Self::Image) {
            Self::Editor
        } else {
            self
        };
        match Self::CYCLE.iter().position(|f| *f == from) {
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
            Self::Log => "Log",
            Self::Image => "Image",
            Self::LogSearch => "Log search",
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
        assert_eq!(FocusTarget::Help.next(), FocusTarget::Editor);
    }

    /// A diff is in the editor's pane, so the cycle steps out of it to where
    /// it would step out of the editor. A history is the same pane again.
    #[test]
    fn cycling_out_of_a_diff_or_a_log_goes_where_the_editor_would() {
        assert_eq!(FocusTarget::Diff.next(), FocusTarget::Explorer);
        assert_eq!(FocusTarget::Log.next(), FocusTarget::Explorer);
        assert_eq!(FocusTarget::LogSearch.next(), FocusTarget::Explorer);
        assert_eq!(FocusTarget::Image.next(), FocusTarget::Explorer);
    }
}
