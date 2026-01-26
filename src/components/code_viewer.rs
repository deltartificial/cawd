use crate::action::Action;
use crate::components::Component;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

pub struct CodeViewer {
    content: Vec<String>,
    highlighted_lines: Vec<Line<'static>>,
    file_path: Option<PathBuf>,
    scroll_offset: usize,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    search_mode: bool,
    search_query: String,
    search_matches: Vec<usize>,
    current_match: usize,
}

impl CodeViewer {
    pub fn new() -> Self {
        Self {
            content: Vec::new(),
            highlighted_lines: Vec::new(),
            file_path: None,
            scroll_offset: 0,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            current_match: 0,
        }
    }

    pub fn load_file(&mut self, path: PathBuf) -> color_eyre::Result<()> {
        let content = std::fs::read_to_string(&path)?;
        self.content = content.lines().map(String::from).collect();
        self.file_path = Some(path.clone());
        self.scroll_offset = 0;
        self.search_query.clear();
        self.search_matches.clear();

        self.highlight_content(&path);

        Ok(())
    }

    fn highlight_content(&mut self, path: &PathBuf) {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt");

        let syntax = self
            .syntax_set
            .find_syntax_by_extension(extension)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes["base16-ocean.dark"];

        let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

        let content_clone: Vec<String> = self.content.clone();

        self.highlighted_lines = content_clone
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let line_num = format!("{:>4} │ ", idx + 1);
                let mut spans: Vec<Span<'static>> = vec![Span::styled(
                    line_num,
                    Style::default().fg(Color::DarkGray),
                )];

                if let Ok(highlighted) = highlighter.highlight_line(line, &self.syntax_set) {
                    for (style, text) in highlighted {
                        let fg = Color::Rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        );

                        let mut ratatui_style = Style::default().fg(fg);

                        if style.font_style.contains(FontStyle::BOLD) {
                            ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
                        }
                        if style.font_style.contains(FontStyle::ITALIC) {
                            ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
                        }
                        if style.font_style.contains(FontStyle::UNDERLINE) {
                            ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
                        }

                        spans.push(Span::styled(text.to_string(), ratatui_style));
                    }
                } else {
                    spans.push(Span::raw(line.clone()));
                }

                Line::from(spans)
            })
            .collect();
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.content.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.content.len().saturating_sub(1);
    }

    fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_match = 0;
    }

    fn exit_search_mode(&mut self) {
        self.search_mode = false;
    }

    fn search_input(&mut self, c: char) {
        self.search_query.push(c);
        self.update_search_matches();
    }

    fn search_backspace(&mut self) {
        self.search_query.pop();
        self.update_search_matches();
    }

    fn update_search_matches(&mut self) {
        self.search_matches.clear();
        self.current_match = 0;

        if self.search_query.is_empty() {
            return;
        }

        let query = self.search_query.to_lowercase();
        for (i, line) in self.content.iter().enumerate() {
            if line.to_lowercase().contains(&query) {
                self.search_matches.push(i);
            }
        }

        if !self.search_matches.is_empty() {
            self.scroll_offset = self.search_matches[0];
        }
    }

    fn next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_match = (self.current_match + 1) % self.search_matches.len();
        self.scroll_offset = self.search_matches[self.current_match];
    }

    fn prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_match = if self.current_match == 0 {
            self.search_matches.len() - 1
        } else {
            self.current_match - 1
        };
        self.scroll_offset = self.search_matches[self.current_match];
    }

    pub fn is_search_mode(&self) -> bool {
        self.search_mode
    }

    pub fn has_file(&self) -> bool {
        self.file_path.is_some()
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
    }

    fn render_welcome(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Welcome ");

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);

        let logo = vec![
            Line::from(""),
            Line::from(""),
            Line::from(vec![
                Span::styled("   ██████╗ █████╗ ██╗    ██╗██████╗ ", Style::default().fg(orange)),
            ]),
            Line::from(vec![
                Span::styled("  ██╔════╝██╔══██╗██║    ██║██╔══██╗", Style::default().fg(orange)),
            ]),
            Line::from(vec![
                Span::styled("  ██║     ███████║██║ █╗ ██║██║  ██║", Style::default().fg(dark_orange)),
            ]),
            Line::from(vec![
                Span::styled("  ██║     ██╔══██║██║███╗██║██║  ██║", Style::default().fg(dark_orange)),
            ]),
            Line::from(vec![
                Span::styled("  ╚██████╗██║  ██║╚███╔███╔╝██████╔╝", Style::default().fg(orange)),
            ]),
            Line::from(vec![
                Span::styled("   ╚═════╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚═════╝ ", Style::default().fg(orange)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Code Viewer with Syntax Highlighting", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(""),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Quick Start", Style::default().fg(orange).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Enter ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Open file / Expand folder", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  /     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Search files in tree", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  j/k   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Navigate up/down", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  h/l   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Collapse/Expand folder", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  .     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Toggle hidden files", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  Tab   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Switch panel", Style::default().fg(Color::White)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  In Code View", Style::default().fg(orange).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  /     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Search in file", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  n/N   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Next/Previous match", Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  g/G   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
                Span::styled("Go to top/bottom", Style::default().fg(Color::White)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  q     ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::styled("Quit", Style::default().fg(Color::White)),
            ]),
        ];

        let paragraph = Paragraph::new(logo).alignment(Alignment::Left);
        frame.render_widget(paragraph, inner);
    }
}

impl Component for CodeViewer {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.search_mode {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.exit_search_mode();
                    Action::None
                }
                KeyCode::Backspace => {
                    self.search_backspace();
                    Action::None
                }
                KeyCode::Char(c) => {
                    self.search_input(c);
                    Action::None
                }
                KeyCode::Up => {
                    self.prev_match();
                    Action::None
                }
                KeyCode::Down => {
                    self.next_match();
                    Action::None
                }
                _ => Action::None,
            }
        } else {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.scroll_up(1);
                    Action::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll_down(1);
                    Action::None
                }
                KeyCode::PageUp => {
                    self.scroll_up(20);
                    Action::None
                }
                KeyCode::PageDown => {
                    self.scroll_down(20);
                    Action::None
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    self.scroll_to_top();
                    Action::None
                }
                KeyCode::End | KeyCode::Char('G') => {
                    self.scroll_to_bottom();
                    Action::None
                }
                KeyCode::Char('/') => {
                    if self.file_path.is_some() {
                        self.enter_search_mode();
                    }
                    Action::None
                }
                KeyCode::Char('n') => {
                    self.next_match();
                    Action::None
                }
                KeyCode::Char('N') => {
                    self.prev_match();
                    Action::None
                }
                _ => Action::None,
            }
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if self.file_path.is_none() {
            self.render_welcome(frame, area, focused);
            return;
        }

        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|n| format!(" {} ", n))
            .unwrap_or_else(|| " Code ".to_string());

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        if self.search_mode {
            let match_info = if self.search_matches.is_empty() {
                "No matches".to_string()
            } else {
                format!("{}/{}", self.current_match + 1, self.search_matches.len())
            };
            let search_title = format!(" /{} [{}] ", self.search_query, match_info);
            block = block.title_bottom(Line::from(search_title).style(Style::default().fg(Color::Yellow)));
        } else if !self.search_query.is_empty() && !self.search_matches.is_empty() {
            let match_info = format!(" {}/{} matches ", self.current_match + 1, self.search_matches.len());
            block = block.title_bottom(Line::from(match_info).style(Style::default().fg(Color::Rgb(0xff, 0x7a, 0x5c))));
        }

        let highlight_style = Style::default()
            .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
            .fg(Color::Rgb(0x1a, 0x12, 0x0f));

        let visible_lines: Vec<Line> = self
            .highlighted_lines
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .map(|(idx, line)| {
                if self.search_matches.contains(&idx) {
                    Line::from(vec![Span::styled(
                        line.to_string(),
                        highlight_style,
                    )])
                } else {
                    line.clone()
                }
            })
            .collect();

        let paragraph = Paragraph::new(visible_lines).block(block);
        frame.render_widget(paragraph, area);
    }
}
