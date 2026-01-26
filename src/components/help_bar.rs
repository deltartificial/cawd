use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub struct HelpBar {
    search_mode: bool,
    in_code_viewer: bool,
}

impl HelpBar {
    pub fn new() -> Self {
        Self {
            search_mode: false,
            in_code_viewer: false,
        }
    }

    pub fn set_context(&mut self, search_mode: bool, in_code_viewer: bool) {
        self.search_mode = search_mode;
        self.in_code_viewer = in_code_viewer;
    }

    #[allow(dead_code)]
    pub fn set_search_mode(&mut self, search_mode: bool) {
        self.search_mode = search_mode;
    }

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
                ("/", "Search"),
                ("n/N", "Next/Prev"),
                ("g/G", "Top/Bottom"),
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

        let mut spans: Vec<Span> = Vec::new();

        for (i, (key, desc)) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", separator_style));
            }
            spans.push(Span::styled(format!(" {} ", key), key_style));
            spans.push(Span::styled(format!(" {}", desc), desc_style));
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(orange_bg));

        frame.render_widget(paragraph, area);
    }
}

impl Default for HelpBar {
    fn default() -> Self {
        Self::new()
    }
}
