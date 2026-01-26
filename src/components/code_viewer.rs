//! Code viewer component with syntax highlighting and search.

use crate::action::Action;
use crate::components::Component;
use crossterm::event::{KeyCode, KeyEvent};

/// The display mode for the code viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Normal syntax-highlighted code view.
    #[default]
    Code,
    /// Git diff view with additions/deletions highlighted.
    Diff,
}
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::path::{Path, PathBuf};
use std::process::Command;
use syntect::highlighting::FontStyle;

/// Code viewer component with syntax highlighting.
///
/// Displays file contents with syntax-aware highlighting, scrolling,
/// and in-file search with match navigation.
pub struct CodeViewer {
    content: Vec<String>,
    highlighted_lines: Vec<Line<'static>>,
    file_path: Option<PathBuf>,
    scroll_offset: usize,
    syntax_set: syntect::parsing::SyntaxSet,
    theme_set: syntect::highlighting::ThemeSet,
    search_mode: bool,
    search_query: String,
    search_matches: Vec<usize>,
    current_match: usize,
    search_list_state: ListState,
    animation_frame: u64,
    view_mode: ViewMode,
}

impl CodeViewer {
    /// Creates a new code viewer instance.
    pub fn new() -> Self {
        Self {
            content: Vec::new(),
            highlighted_lines: Vec::new(),
            file_path: None,
            scroll_offset: 0,
            syntax_set: two_face::syntax::extra_newlines(),
            theme_set: two_face::theme::extra().into(),
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            current_match: 0,
            search_list_state: ListState::default(),
            animation_frame: 0,
            view_mode: ViewMode::default(),
        }
    }

    /// Loads a file and applies syntax highlighting.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the file to load.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if the file cannot be read.
    pub fn load_file(&mut self, path: PathBuf) -> color_eyre::Result<()> {
        let content = std::fs::read_to_string(&path)?;
        self.content = content.lines().map(String::from).collect();
        self.file_path = Some(path.clone());
        self.scroll_offset = 0;
        self.search_query.clear();
        self.search_matches.clear();
        self.view_mode = ViewMode::Code;

        self.highlight_content(&path);

        Ok(())
    }

    /// Displays an error message in the viewer.
    ///
    /// # Parameters
    ///
    /// * `message` - The error message to display.
    pub fn show_error(&mut self, message: &str) {
        self.content = vec![
            String::new(),
            format!("  Error: {}", message),
            String::new(),
        ];
        self.file_path = None;
        self.scroll_offset = 0;
        self.view_mode = ViewMode::Code;
        self.highlighted_lines = self
            .content
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                Line::from(vec![
                    Span::styled(
                        format!("{:>4} │ ", idx + 1),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(line.clone(), Style::default().fg(Color::Red)),
                ])
            })
            .collect();
    }

    /// Loads a git diff for the specified file.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the file to diff.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if git diff fails.
    pub fn load_diff(&mut self, path: PathBuf) -> color_eyre::Result<()> {
        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .arg(&path)
            .current_dir(path.parent().unwrap_or(&path))
            .output()?;

        let diff_content = String::from_utf8_lossy(&output.stdout);

        if diff_content.is_empty() {
            let staged_output = Command::new("git")
                .args(["diff", "--cached"])
                .arg(&path)
                .current_dir(path.parent().unwrap_or(&path))
                .output()?;

            let staged_diff = String::from_utf8_lossy(&staged_output.stdout);
            if staged_diff.is_empty() {
                self.content = vec!["No changes to display".to_string()];
            } else {
                self.content = staged_diff.lines().map(String::from).collect();
            }
        } else {
            self.content = diff_content.lines().map(String::from).collect();
        }

        self.file_path = Some(path);
        self.scroll_offset = 0;
        self.search_query.clear();
        self.search_matches.clear();
        self.view_mode = ViewMode::Diff;

        self.highlight_diff();

        Ok(())
    }

    /// Applies diff-specific highlighting with colored backgrounds.
    fn highlight_diff(&mut self) {
        let green_bg = Color::Rgb(0x1a, 0x3d, 0x1a);
        let green_fg = Color::Rgb(0x80, 0xff, 0x80);
        let red_bg = Color::Rgb(0x3d, 0x1a, 0x1a);
        let red_fg = Color::Rgb(0xff, 0x80, 0x80);

        // Take ownership to avoid clone allocations
        let content = std::mem::take(&mut self.content);

        let filtered: Vec<String> = content
            .into_iter()
            .filter(|line| {
                !line.starts_with("diff ")
                    && !line.starts_with("index ")
                    && !line.starts_with("---")
                    && !line.starts_with("+++")
                    && !line.starts_with("@@")
                    && !line.starts_with("\\")
            })
            .collect();

        let mut line_num = 0usize;
        self.highlighted_lines = filtered
            .iter()
            .map(|line| {
                line_num += 1;
                let num_str = format!("{:>4} │ ", line_num);
                let content = if line.len() > 1 { &line[1..] } else { "" };

                if line.starts_with('+') {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(green_fg).bg(green_bg)),
                        Span::styled(format!("+ {}", content), Style::default().fg(green_fg).bg(green_bg)),
                    ])
                } else if line.starts_with('-') {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(red_fg).bg(red_bg)),
                        Span::styled(format!("- {}", content), Style::default().fg(red_fg).bg(red_bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("  {}", content), Style::default().fg(Color::White)),
                    ])
                }
            })
            .collect();

        self.content = filtered;
    }

    /// Applies syntax highlighting to the loaded content.
    fn highlight_content(&mut self, path: &Path) {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt");

        let mapped_extension = match extension {
            "sol" => "rs",
            "vyper" | "vy" => "py",
            "cairo" => "rs",
            "move" => "rs",
            ext => ext,
        };

        let syntax = self
            .syntax_set
            .find_syntax_by_extension(mapped_extension)
            .or_else(|| self.syntax_set.find_syntax_by_extension(extension))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let Some(theme) = self.theme_set.themes.get("base16-ocean.dark") else {
            self.highlighted_lines = self.content.iter().enumerate().map(|(idx, line)| {
                Line::from(vec![
                    Span::styled(format!("{:>4} │ ", idx + 1), Style::default().fg(Color::DarkGray)),
                    Span::raw(line.clone()),
                ])
            }).collect();
            return;
        };

        let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

        self.highlighted_lines = self
            .content
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

    /// Scrolls the view up by the specified amount.
    fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scrolls the view down by the specified amount.
    fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.content.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    /// Scrolls to the top of the file.
    fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scrolls to the bottom of the file.
    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.content.len().saturating_sub(1);
    }

    /// Enters search mode.
    fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_match = 0;
    }

    /// Exits search mode.
    fn exit_search_mode(&mut self) {
        self.search_mode = false;
    }

    /// Appends a character to the search query.
    fn search_input(&mut self, c: char) {
        self.search_query.push(c);
        self.update_search_matches();
    }

    /// Removes the last character from the search query.
    fn search_backspace(&mut self) {
        self.search_query.pop();
        self.update_search_matches();
    }

    /// Updates the list of matching line numbers.
    fn update_search_matches(&mut self) {
        self.search_matches.clear();
        self.current_match = 0;

        if self.search_query.is_empty() {
            self.search_list_state.select(None);
            return;
        }

        let query = self.search_query.to_lowercase();
        for (i, line) in self.content.iter().enumerate() {
            if line.to_lowercase().contains(&query) {
                self.search_matches.push(i);
            }
        }

        if let Some(&first_match) = self.search_matches.first() {
            self.scroll_offset = first_match;
            self.search_list_state.select(Some(0));
        } else {
            self.search_list_state.select(None);
        }
    }

    /// Navigates to the next search match.
    fn next_match(&mut self) {
        let len = self.search_matches.len();
        if len == 0 {
            return;
        }
        self.current_match = (self.current_match + 1) % len;
        if let Some(&offset) = self.search_matches.get(self.current_match) {
            self.scroll_offset = offset;
        }
        self.search_list_state.select(Some(self.current_match));
    }

    /// Navigates to the previous search match.
    fn prev_match(&mut self) {
        let len = self.search_matches.len();
        if len == 0 {
            return;
        }
        self.current_match = self.current_match.checked_sub(1).unwrap_or(len.saturating_sub(1));
        if let Some(&offset) = self.search_matches.get(self.current_match) {
            self.scroll_offset = offset;
        }
        self.search_list_state.select(Some(self.current_match));
    }

    /// Returns whether the component is in search mode.
    pub fn is_search_mode(&self) -> bool {
        self.search_mode
    }

    /// Returns whether a file is currently loaded.
    pub fn has_file(&self) -> bool {
        self.file_path.is_some()
    }

    /// Returns the name of the currently loaded file, if any.
    pub fn file_name(&self) -> Option<&str> {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
    }

    /// Renders the welcome screen with animated logo.
    fn render_welcome(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        self.animation_frame = self.animation_frame.wrapping_add(1);

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

        let frame_offset = self.animation_frame as f64 * 0.08;

        let logo_lines = [
            "   ██████╗ █████╗ ██╗    ██╗██████╗ ",
            "  ██╔════╝██╔══██╗██║    ██║██╔══██╗",
            "  ██║     ███████║██║ █╗ ██║██║  ██║",
            "  ██║     ██╔══██║██║███╗██║██║  ██║",
            "  ╚██████╗██║  ██║╚███╔███╔╝██████╔╝",
            "   ╚═════╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚═════╝ ",
        ];

        let mut logo: Vec<Line<'static>> = Vec::with_capacity(30);
        logo.push(Line::from(""));
        logo.push(Line::from(""));

        for (i, line_text) in logo_lines.iter().enumerate() {
            let phase = frame_offset + (i as f64 * 0.6);
            let wave = (phase.sin() + 1.0) / 2.0;

            let r = (0xaa as f64 + (0xff - 0xaa) as f64 * wave) as u8;
            let g = (0x44 as f64 + (0x7a - 0x44) as f64 * wave) as u8;
            let b = (0x22 as f64 + (0x5c - 0x22) as f64 * wave) as u8;

            let color = Color::Rgb(r, g, b);
            logo.push(Line::from(Span::styled(
                line_text.to_string(),
                Style::default().fg(color),
            )));
        }

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);

        logo.push(Line::from(""));
        logo.push(Line::from(Span::styled(
            "  Code Aware Workspace Display",
            Style::default().fg(Color::DarkGray),
        )));
        logo.push(Line::from(""));
        logo.push(Line::from(""));
        logo.push(Line::from(Span::styled(
            "  Quick Start",
            Style::default().fg(orange).add_modifier(Modifier::BOLD),
        )));
        logo.push(Line::from(""));

        logo.push(Line::from(vec![
            Span::styled("  Enter ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Open file / Expand folder", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  /     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Search files in tree", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  j/k   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Navigate up/down", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  h/l   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Collapse/Expand folder", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  .     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Toggle hidden files", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  Tab   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Switch panel", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(""));
        logo.push(Line::from(Span::styled(
            "  In Code View",
            Style::default().fg(orange).add_modifier(Modifier::BOLD),
        )));
        logo.push(Line::from(""));
        logo.push(Line::from(vec![
            Span::styled("  /     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Search in file", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  n/N   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Next/Previous match", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  g/G   ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Go to top/bottom", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(""));
        logo.push(Line::from(vec![
            Span::styled("  q     ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled("Quit", Style::default().fg(Color::White)),
        ]));

        let paragraph = Paragraph::new(logo).alignment(Alignment::Left);
        frame.render_widget(paragraph, inner);
    }

    /// Renders the in-file search modal.
    fn render_search_modal_static(
        frame: &mut Frame,
        area: Rect,
        search_query: &str,
        search_matches: &[usize],
        current_match: usize,
        content: &[String],
        search_list_state: &mut ListState,
    ) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);
        let dark_bg = Color::Rgb(0x1a, 0x1a, 0x2e);
        let dark_text = Color::Rgb(0x1a, 0x12, 0x0f);

        let modal_width = (area.width as f32 * 0.7).min(70.0) as u16;
        let modal_height = (area.height as f32 * 0.5).min(20.0) as u16;

        let modal_area = Rect {
            x: area.x + (area.width - modal_width) / 2,
            y: area.y + (area.height - modal_height) / 2,
            width: modal_width,
            height: modal_height,
        };

        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(orange))
            .style(Style::default().bg(dark_bg))
            .title(Line::from(vec![
                Span::styled(" \u{f002} ", Style::default().fg(orange)),
                Span::styled("Search in File", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
            ]));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(inner);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .style(Style::default().bg(dark_bg));

        let cursor = "▌";
        let match_info = if search_matches.is_empty() {
            if search_query.is_empty() {
                String::new()
            } else {
                " (no matches)".to_string()
            }
        } else {
            format!(" ({}/{})", current_match + 1, search_matches.len())
        };

        let input_text = Line::from(vec![
            Span::styled("\u{f002} ", Style::default().fg(orange)),
            Span::raw(search_query),
            Span::styled(cursor, Style::default().fg(orange)),
            Span::styled(match_info, Style::default().fg(Color::DarkGray)),
        ]);

        let input = Paragraph::new(input_text)
            .block(input_block)
            .style(Style::default().fg(Color::White));

        frame.render_widget(input, layout[0]);

        let items: Vec<ListItem> = search_matches
            .iter()
            .take(15)
            .map(|&line_idx| {
                let line_num = format!("{:>4}", line_idx + 1);
                let line_content = content.get(line_idx).map(|s| s.trim()).unwrap_or_default();
                let truncated: String = if line_content.chars().count() > 50 {
                    format!("{}...", line_content.chars().take(50).collect::<String>())
                } else {
                    line_content.to_string()
                };

                let line = Line::from(vec![
                    Span::styled(format!("{} ", line_num), Style::default().fg(orange)),
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(truncated, Style::default().fg(Color::White)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let highlight_style = Style::default()
            .bg(dark_orange)
            .fg(dark_text)
            .add_modifier(Modifier::BOLD);

        let results_block = Block::default().style(Style::default().bg(dark_bg));

        let list = List::new(items)
            .block(results_block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, layout[1], search_list_state);
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
            .map(|n| {
                if self.view_mode == ViewMode::Diff {
                    format!(" [DIFF] {} ", n)
                } else {
                    format!(" {} ", n)
                }
            })
            .unwrap_or_else(|| " Code ".to_string());

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        if !self.search_mode && !self.search_query.is_empty() && !self.search_matches.is_empty() {
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

        if self.search_mode {
            Self::render_search_modal_static(
                frame,
                area,
                &self.search_query,
                &self.search_matches,
                self.current_match,
                &self.content,
                &mut self.search_list_state,
            );
        }
    }
}
