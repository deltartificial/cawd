//! Application actions that can be triggered by user input or internal events.

use std::path::PathBuf;

/// Represents all possible actions that can occur in the application.
///
/// Actions are returned by components when handling key events and are
/// processed by the main application to update state or trigger behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Action {
    /// Exit the application.
    Quit,

    /// Switch focus to the next panel.
    SwitchPanel,

    /// Navigate up in a list or tree.
    Up,

    /// Navigate down in a list or tree.
    Down,

    /// Navigate left or collapse a tree node.
    Left,

    /// Navigate right or expand a tree node.
    Right,

    /// Confirm selection or action.
    Enter,

    /// Scroll up by a page.
    PageUp,

    /// Scroll down by a page.
    PageDown,

    /// Jump to the beginning.
    Home,

    /// Jump to the end.
    End,

    /// A file was selected for viewing.
    ///
    /// # Parameters
    ///
    /// * `PathBuf` - The absolute path to the selected file.
    FileSelected(PathBuf),

    /// Toggle visibility of hidden files.
    ToggleHidden,

    /// Enter search/filter mode.
    EnterSearchMode,

    /// Exit search/filter mode.
    ExitSearchMode,

    /// Add a character to the search query.
    ///
    /// # Parameters
    ///
    /// * `char` - The character to append.
    SearchInput(char),

    /// Remove the last character from the search query.
    SearchBackspace,

    /// Confirm and execute the search.
    SearchConfirm,

    /// No action required.
    None,
}
