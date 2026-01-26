pub mod code_viewer;
pub mod file_tree;
pub mod help_bar;
pub mod search;
pub mod search_modal;

use crate::action::Action;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

/// Component trait for UI elements
pub trait Component {
    /// Handle key events and return an optional action
    fn handle_key_event(&mut self, key: KeyEvent) -> Action;

    /// Render the component
    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool);
}
