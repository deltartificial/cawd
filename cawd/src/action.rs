//! Application actions that can be triggered by user input or internal events.

use std::path::PathBuf;

/// Represents all possible actions that can occur in the application.
///
/// Actions are returned by components when handling key events and are
/// processed by the main application to update state or trigger behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// A file was selected for viewing.
    ///
    /// # Parameters
    ///
    /// * `PathBuf` - The absolute path to the selected file.
    FileSelected(PathBuf),

    /// A file was selected for diff viewing (from git status).
    ///
    /// # Parameters
    ///
    /// * `PathBuf` - The absolute path to the file to diff.
    DiffSelected(PathBuf),

    /// A commit was selected in the Changes panel; show its full diff.
    ///
    /// Carries the abbreviated commit hash.
    CommitSelected(String),

    /// An annotation was opened from the review panel.
    ///
    /// Loads the annotated file in the code viewer and scrolls to the line,
    /// keeping focus on the review panel.
    AnnotationOpen {
        /// Absolute path to the annotated file.
        path: PathBuf,
        /// 1-based line to scroll to.
        line: usize,
    },

    /// Save-and-dispatch: an annotation was just created from the comment
    /// dialog and a worker should be launched on it immediately.
    DispatchWorker {
        /// Id of the freshly saved annotation.
        id: String,
        /// When true, the worker commits and pushes its changes once it
        /// finishes successfully.
        commit: bool,
    },

    /// Open a URL in the user's default browser.
    ///
    /// Emitted by the Notion panel when a ticket is selected.
    OpenUrl(String),

    /// Toggle visibility of hidden files.
    ToggleHidden,

    /// Enter search/filter mode.
    EnterSearchMode,

    /// Exit search/filter mode.
    ExitSearchMode,

    /// No action required.
    None,
}
