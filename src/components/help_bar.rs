//! Context-sensitive help bar displaying available keybindings.

use chrono::Local;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Help bar component showing keyboard shortcuts.
///
/// Displays different keybindings based on the current context
/// (file tree, code viewer, git status, or search mode).
pub struct HelpBar {
    search_mode: bool,
    in_code_viewer: bool,
    in_git_status: bool,
    in_review: bool,
}

impl HelpBar {
    /// Creates a new help bar instance.
    pub fn new() -> Self {
        Self {
            search_mode: false,
            in_code_viewer: false,
            in_git_status: false,
            in_review: false,
        }
    }

    /// Updates the context for determining which keybindings to show.
    ///
    /// # Parameters
    ///
    /// * `search_mode` - Whether any component is in search mode.
    /// * `in_code_viewer` - Whether the code viewer is focused with a file open.
    /// * `in_git_status` - Whether the git status panel is focused.
    /// * `in_review` - Whether the review panel is focused.
    pub fn set_context(
        &mut self,
        search_mode: bool,
        in_code_viewer: bool,
        in_git_status: bool,
        in_review: bool,
    ) {
        self.search_mode = search_mode;
        self.in_code_viewer = in_code_viewer;
        self.in_git_status = in_git_status;
        self.in_review = in_review;
    }

    /// Sets the search mode flag.
    pub fn set_search_mode(&mut self, search_mode: bool) {
        self.search_mode = search_mode;
    }

    /// Renders the help bar to the terminal frame.
    ///
    /// # Parameters
    ///
    /// * `frame` - The terminal frame to render to.
    /// * `area` - The rectangular area allocated for this component.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let orange_bg = Color::Rgb(0xe6, 0x5a, 0x3d);
        let dark_text = Color::Rgb(0x1a, 0x12, 0x0f);
        let light_orange = Color::Rgb(0xff, 0x7a, 0x5c);

        let key_style = Style::default()
            .fg(dark_text)
            .bg(light_orange)
            .add_modifier(Modifier::BOLD);

        let desc_style = Style::default().fg(dark_text);
        let separator_style = Style::default().fg(Color::Rgb(0x6b, 0x5a, 0x52));

        let items: Vec<(&str, &str)> = if self.search_mode {
            vec![
                ("↑/↓", "Navigate"),
                ("Enter", "Select"),
                ("Esc", "Cancel"),
                ("⌫", "Delete"),
            ]
        } else if self.in_code_viewer {
            vec![
                ("j/k", "Scroll"),
                ("drag", "Select"),
                ("c", "Comment"),
                ("/", "Search"),
                ("g/G", "Top/Bottom"),
                ("Tab", "Panel"),
                ("q", "Quit"),
            ]
        } else if self.in_review {
            vec![
                ("j/k", "Navigate"),
                ("Enter", "Open"),
                ("w", "Worker"),
                ("s", "Status"),
                ("a", "Show all"),
                ("d", "Delete"),
                ("Tab", "Panel"),
                ("q", "Quit"),
            ]
        } else if self.in_git_status {
            vec![
                ("j/k", "Navigate"),
                ("Enter", "Open"),
                ("/", "Search"),
                ("r", "Refresh"),
                ("Tab", "Panel"),
                ("q", "Quit"),
            ]
        } else {
            vec![
                ("j/k", "Navigate"),
                ("Enter", "Open"),
                ("h/l", "Collapse/Expand"),
                ("/", "Search"),
                (".", "Hidden"),
                ("Tab", "Panel"),
                ("q", "Quit"),
            ]
        };

        let mut spans: Vec<Span> = Vec::with_capacity(items.len() * 3);

        for (i, (key, desc)) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", separator_style));
            }
            spans.push(Span::styled(format!(" {} ", key), key_style));
            spans.push(Span::styled(format!(" {}", desc), desc_style));
        }

        let time_str = Local::now().format(" %H:%M:%S ").to_string();
        let time_width = time_str.len() as u16;

        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(time_width),
            ])
            .split(area);

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(orange_bg));
        frame.render_widget(paragraph, layout[0]);

        let time_line = Line::from(vec![
            Span::styled(time_str, key_style),
        ]);
        let time_paragraph = Paragraph::new(time_line).style(Style::default().bg(orange_bg));
        frame.render_widget(time_paragraph, layout[1]);
    }
}

impl Default for HelpBar {
    fn default() -> Self {
        Self::new()
    }
}
