//! Global file search modal with fuzzy matching.

use crossterm::event::{KeyCode, KeyEvent};
use devicons::FileIcon;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// The type of search to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchType {
    /// Search for files by name.
    Files,
    /// Search for content within files (grep-like).
    Grep,
}

/// Represents a search result with display information.
#[derive(Debug, Clone)]
pub(crate) struct SearchResult {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Display name (filename only).
    pub name: String,
    /// Whether this is a directory.
    pub is_dir: bool,
    /// File type icon.
    pub icon: String,
    /// Color for the icon.
    pub color: Color,
    /// Path relative to the search root.
    pub relative_path: String,
}

impl SearchResult {
    /// Creates a new search result from a path.
    ///
    /// # Parameters
    ///
    /// * `path` - The absolute path to the file.
    /// * `root` - The root directory for computing relative paths.
    pub(crate) fn new(path: PathBuf, root: &PathBuf) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

        let is_dir = path.is_dir();

        let (icon, color) = if is_dir {
            ("\u{f07b}".to_owned(), Color::Rgb(0xff, 0x7a, 0x5c))
        } else {
            let file_icon = FileIcon::from(&name);
            (file_icon.icon.to_string(), Self::devicon_color_to_ratatui(file_icon.color))
        };

        let relative_path = path.strip_prefix(root).map_or_else(
            |_| path.to_string_lossy().to_string(),
            |p| p.to_string_lossy().to_string(),
        );

        Self { path, name, is_dir, icon, color, relative_path }
    }

    /// Converts a hex color string to a ratatui Color.
    fn devicon_color_to_ratatui(hex: &str) -> Color {
        if hex.starts_with('#') &&
            hex.len() == 7 &&
            let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[1..3], 16),
                u8::from_str_radix(&hex[3..5], 16),
                u8::from_str_radix(&hex[5..7], 16),
            )
        {
            return Color::Rgb(r, g, b);
        }
        Color::White
    }
}

/// Global file search modal component.
///
/// Provides a fuzzy file finder that indexes the project directory
/// and allows quick navigation to any file.
pub(crate) struct SearchModal {
    /// Whether the modal is currently visible.
    pub active: bool,
    /// The current search query.
    pub query: String,
    /// The type of search being performed.
    pub search_type: SearchType,
    /// Current search results.
    pub results: Vec<SearchResult>,
    /// Selection state for the results list.
    pub list_state: ListState,
    root: PathBuf,
    all_files: Vec<PathBuf>,
}

impl SearchModal {
    /// Creates a new search modal for the given root directory.
    ///
    /// # Parameters
    ///
    /// * `root` - The root directory to index for searching.
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut modal = Self {
            active: false,
            query: String::new(),
            search_type: SearchType::Files,
            results: Vec::new(),
            list_state: ListState::default(),
            root: root.clone(),
            all_files: Vec::new(),
        };
        modal.index_files(&root);
        modal
    }

    /// Indexes all files in the directory tree.
    fn index_files(&mut self, root: &Path) {
        self.all_files.clear();
        self.index_recursive(root, &mut BTreeSet::new());
    }

    /// Recursively indexes files, avoiding cycles and ignored directories.
    fn index_recursive(&mut self, dir: &Path, visited: &mut BTreeSet<PathBuf>) {
        if let Ok(canonical) = dir.canonicalize() {
            if visited.contains(&canonical) {
                return;
            }
            visited.insert(canonical);
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name =
                    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

                if name.starts_with('.') ||
                    name == "node_modules" ||
                    name == "target" ||
                    name == "__pycache__" ||
                    name == "dist" ||
                    name == "build"
                {
                    continue;
                }

                self.all_files.push(path.clone());

                if path.is_dir() && self.all_files.len() < 10000 {
                    self.index_recursive(&path, visited);
                }
            }
        }
    }

    /// Opens the search modal.
    pub(crate) fn open(&mut self) {
        self.active = true;
        self.query.clear();
        self.results.clear();
        self.list_state.select(Some(0));
        self.update_results();
    }

    /// Closes the search modal.
    pub(crate) fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
    }

    /// Handles a key event while the modal is active.
    ///
    /// # Parameters
    ///
    /// * `key` - The key event to handle.
    ///
    /// # Returns
    ///
    /// Returns `Some(PathBuf)` if a file was selected, `None` otherwise.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode/MouseEventKind are non_exhaustive, a catch-all arm is required"
    )]
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        match key.code {
            KeyCode::Esc => {
                self.close();
                None
            }
            KeyCode::Enter => {
                if let Some(selected) = self.list_state.selected() &&
                    let Some(result) = self.results.get(selected)
                {
                    let path = result.path.clone();
                    self.close();
                    return Some(path);
                }
                None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.update_results();
                None
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.update_results();
                None
            }
            KeyCode::Up => {
                self.move_up();
                None
            }
            KeyCode::Down => {
                self.move_down();
                None
            }
            KeyCode::Tab => {
                self.search_type = match self.search_type {
                    SearchType::Files => SearchType::Grep,
                    SearchType::Grep => SearchType::Files,
                };
                self.update_results();
                None
            }
            _ => None,
        }
    }

    /// Moves selection up in the results list.
    fn move_up(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or_default();
        let new_idx = if current == 0 { self.results.len().saturating_sub(1) } else { current - 1 };
        self.list_state.select(Some(new_idx));
    }

    /// Moves selection down in the results list.
    fn move_down(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or_default();
        let new_idx = if current >= self.results.len().saturating_sub(1) { 0 } else { current + 1 };
        self.list_state.select(Some(new_idx));
    }

    /// Updates the search results based on the current query.
    fn update_results(&mut self) {
        self.results.clear();

        if self.query.is_empty() {
            for path in self.all_files.iter().take(20) {
                self.results.push(SearchResult::new(path.clone(), &self.root));
            }
        } else {
            let query_lower = self.query.to_lowercase();
            let query_chars: Vec<char> = query_lower.chars().collect();

            let mut scored_results: Vec<(SearchResult, i32)> = self
                .all_files
                .iter()
                .filter_map(|path| {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();

                    let score = Self::fuzzy_score(&name, &query_chars);
                    (score > 0).then(|| (SearchResult::new(path.clone(), &self.root), score))
                })
                .collect();

            scored_results.sort_by_key(|r| core::cmp::Reverse(r.1));

            self.results = scored_results.into_iter().take(20).map(|(r, _)| r).collect();
        }

        if self.results.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    /// Computes a fuzzy match score for a filename.
    ///
    /// Higher scores indicate better matches. Returns 0 if no match.
    fn fuzzy_score(text: &str, query: &[char]) -> i32 {
        if query.is_empty() {
            return 1;
        }

        let text_chars: Vec<char> = text.chars().collect();
        let mut score = 0;
        let mut query_idx = 0;
        let mut prev_match_idx: Option<usize> = None;

        for (i, &c) in text_chars.iter().enumerate() {
            if query_idx < query.len() && c == query[query_idx] {
                score += 10;

                if let Some(prev) = prev_match_idx &&
                    i == prev + 1
                {
                    score += 15;
                }

                if i == 0 ||
                    text_chars.get(i - 1).is_some_and(|&c| c == '_' || c == '-' || c == '.')
                {
                    score += 10;
                }

                prev_match_idx = Some(i);
                query_idx += 1;
            }
        }

        if query_idx == query.len() { score } else { 0 }
    }

    /// Renders the search modal to the terminal frame.
    ///
    /// # Parameters
    ///
    /// * `frame` - The terminal frame to render to.
    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        if !self.active {
            return;
        }

        let area = frame.area();

        let modal_width = (f32::from(area.width) * 0.6).min(80.0) as u16;
        let modal_height = (f32::from(area.height) * 0.6).min(30.0) as u16;

        let modal_area = Rect {
            x: (area.width - modal_width) / 2,
            y: (area.height - modal_height) / 2,
            width: modal_width,
            height: modal_height,
        };

        frame.render_widget(Clear, modal_area);

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);
        let dark_bg = Color::Rgb(0x1a, 0x1a, 0x2e);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(orange))
            .style(Style::default().bg(dark_bg))
            .title(Line::from(vec![
                Span::styled(" \u{f002} ", Style::default().fg(orange)),
                Span::styled(
                    "Find Files",
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .style(Style::default().bg(dark_bg));

        let cursor = "▌";
        let input_text = Line::from(vec![
            Span::styled("\u{f002} ", Style::default().fg(orange)),
            Span::raw(&self.query),
            Span::styled(cursor, Style::default().fg(orange)),
        ]);

        let input =
            Paragraph::new(input_text).block(input_block).style(Style::default().fg(Color::White));

        frame.render_widget(input, layout[0]);

        let files_style = if self.search_type == SearchType::Files {
            Style::default().fg(dark_bg).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let grep_style = if self.search_type == SearchType::Grep {
            Style::default().fg(dark_bg).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let tabs = Line::from(vec![
            Span::styled(" \u{f15b} Files ", files_style),
            Span::raw(" "),
            Span::styled(" \u{f002} Grep ", grep_style),
            Span::styled("  (Tab to switch)", Style::default().fg(Color::DarkGray)),
        ]);

        let tabs_widget = Paragraph::new(tabs).style(Style::default().bg(dark_bg));
        frame.render_widget(tabs_widget, layout[1]);

        let results_block = Block::default().style(Style::default().bg(dark_bg));

        let items: Vec<ListItem<'_>> = self
            .results
            .iter()
            .map(|result| {
                let icon_style = Style::default().fg(result.color);
                let name_style = if result.is_dir {
                    Style::default().fg(orange).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(result.color)
                };
                let path_style = Style::default().fg(Color::DarkGray);

                let line = Line::from(vec![
                    Span::styled(format!(" {} ", result.icon), icon_style),
                    Span::styled(&result.name, name_style),
                    Span::styled(format!("  {}", result.relative_path), path_style),
                ]);

                ListItem::new(line)
            })
            .collect();

        let highlight_style = Style::default()
            .bg(dark_orange)
            .fg(Color::Rgb(0x1a, 0x12, 0x0f))
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(results_block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, layout[2], &mut self.list_state);
    }
}
