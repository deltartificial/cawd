//! Legacy search input component (currently unused).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// A simple search input field component.
///
/// Note: This component is currently unused in favor of inline search
/// within other components, but is kept for potential future use.
pub struct SearchInput {
    query: String,
    active: bool,
}

impl SearchInput {
    /// Creates a new search input.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
        }
    }

    /// Sets whether the search input is active.
    ///
    /// Clears the query when deactivated.
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.query.clear();
        }
    }

    /// Returns whether the search input is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Appends a character to the query.
    pub fn push(&mut self, c: char) {
        self.query.push(c);
    }

    /// Removes the last character from the query.
    pub fn pop(&mut self) {
        self.query.pop();
    }

    /// Returns the current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Clears the search query.
    pub fn clear(&mut self) {
        self.query.clear();
    }

    /// Renders the search input to the terminal frame.
    ///
    /// # Parameters
    ///
    /// * `frame` - The terminal frame to render to.
    /// * `area` - The rectangular area allocated for this component.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if !self.active {
            return;
        }

        let border_style = Style::default().fg(Color::Yellow);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Search ");

        let cursor = if self.active { "▌" } else { "" };
        let line = Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::raw(&self.query),
            Span::styled(cursor, Style::default().fg(Color::Yellow).add_modifier(Modifier::SLOW_BLINK)),
        ]);

        let paragraph = Paragraph::new(line).block(block);
        frame.render_widget(paragraph, area);
    }
}

impl Default for SearchInput {
    fn default() -> Self {
        Self::new()
    }
}
