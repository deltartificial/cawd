use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[allow(dead_code)]
pub struct SearchInput {
    query: String,
    active: bool,
}

#[allow(dead_code)]
impl SearchInput {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
        }
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.query.clear();
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop(&mut self) {
        self.query.pop();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn clear(&mut self) {
        self.query.clear();
    }

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
