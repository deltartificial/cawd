//! UI components for the terminal interface.
//!
//! Each component implements the [`Component`] trait for consistent
//! event handling and rendering behavior.

pub mod code_viewer;
pub mod file_tree;
pub mod git_status;
pub mod help_bar;
pub mod search;
pub mod search_modal;

use crate::action::Action;
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

/// Trait for UI components that can handle input and render themselves.
///
/// All interactive panels in the application implement this trait to
/// provide a consistent interface for the main application loop.
pub trait Component {
    /// Handles a key event and returns the resulting action.
    ///
    /// # Parameters
    ///
    /// * `key` - The key event to handle.
    ///
    /// # Returns
    ///
    /// An [`Action`] indicating what the application should do in response.
    fn handle_key_event(&mut self, key: KeyEvent) -> Action;

    /// Renders the component to the terminal frame.
    ///
    /// # Parameters
    ///
    /// * `frame` - The terminal frame to render to.
    /// * `area` - The rectangular area allocated for this component.
    /// * `focused` - Whether this component currently has focus.
    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool);
}
