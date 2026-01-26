use crossterm::event::{KeyCode, KeyEvent};
use devicons::FileIcon;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchType {
    Files,
    Grep,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub icon: String,
    pub color: Color,
    pub relative_path: String,
}

impl SearchResult {
    pub fn new(path: PathBuf, root: &PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let is_dir = path.is_dir();

        let (icon, color) = if is_dir {
            ("\u{f07b}".to_string(), Color::Rgb(0xff, 0x7a, 0x5c))
        } else {
            let file_icon = FileIcon::from(&name);
            (file_icon.icon.to_string(), Self::devicon_color_to_ratatui(&file_icon.color))
        };

        let relative_path = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        Self {
            path,
            name,
            is_dir,
            icon,
            color,
            relative_path,
        }
    }

    fn devicon_color_to_ratatui(hex: &str) -> Color {
        if hex.starts_with('#') && hex.len() == 7 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[1..3], 16),
                u8::from_str_radix(&hex[3..5], 16),
                u8::from_str_radix(&hex[5..7], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
        Color::White
    }
}

pub struct SearchModal {
    pub active: bool,
    pub query: String,
    pub search_type: SearchType,
    pub results: Vec<SearchResult>,
    pub list_state: ListState,
    root: PathBuf,
    all_files: Vec<PathBuf>,
}

impl SearchModal {
    pub fn new(root: PathBuf) -> Self {
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

    fn index_files(&mut self, root: &PathBuf) {
        self.all_files.clear();
        self.index_recursive(root, &mut HashSet::new());
    }

    fn index_recursive(&mut self, dir: &PathBuf, visited: &mut HashSet<PathBuf>) {
        if let Ok(canonical) = dir.canonicalize() {
            if visited.contains(&canonical) {
                return;
            }
            visited.insert(canonical);
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden files and common ignore patterns
                if name.starts_with('.')
                    || name == "node_modules"
                    || name == "target"
                    || name == "__pycache__"
                    || name == "dist"
                    || name == "build"
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

    pub fn open(&mut self) {
        self.active = true;
        self.query.clear();
        self.results.clear();
        self.list_state.select(Some(0));
        self.update_results();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        match key.code {
            KeyCode::Esc => {
                self.close();
                None
            }
            KeyCode::Enter => {
                if let Some(selected) = self.list_state.selected() {
                    if let Some(result) = self.results.get(selected) {
                        let path = result.path.clone();
                        self.close();
                        return Some(path);
                    }
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

    fn move_up(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if current == 0 {
            self.results.len().saturating_sub(1)
        } else {
            current - 1
        };
        self.list_state.select(Some(new_idx));
    }

    fn move_down(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if current >= self.results.len().saturating_sub(1) {
            0
        } else {
            current + 1
        };
        self.list_state.select(Some(new_idx));
    }

    fn update_results(&mut self) {
        self.results.clear();

        if self.query.is_empty() {
            // Show recent/all files when no query
            for path in self.all_files.iter().take(20) {
                self.results.push(SearchResult::new(path.clone(), &self.root));
            }
        } else {
            let query_lower = self.query.to_lowercase();
            let query_chars: Vec<char> = query_lower.chars().collect();

            // Fuzzy search
            let mut scored_results: Vec<(SearchResult, i32)> = self
                .all_files
                .iter()
                .filter_map(|path| {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();

                    let score = self.fuzzy_score(&name, &query_chars);
                    if score > 0 {
                        Some((SearchResult::new(path.clone(), &self.root), score))
                    } else {
                        None
                    }
                })
                .collect();

            // Sort by score (higher is better)
            scored_results.sort_by(|a, b| b.1.cmp(&a.1));

            self.results = scored_results
                .into_iter()
                .take(20)
                .map(|(r, _)| r)
                .collect();
        }

        // Reset selection
        if !self.results.is_empty() {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }
    }

    fn fuzzy_score(&self, text: &str, query: &[char]) -> i32 {
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

                // Bonus for consecutive matches
                if let Some(prev) = prev_match_idx {
                    if i == prev + 1 {
                        score += 15;
                    }
                }

                // Bonus for start of word
                if i == 0 || text_chars.get(i - 1).map(|&c| c == '_' || c == '-' || c == '.').unwrap_or(false) {
                    score += 10;
                }

                prev_match_idx = Some(i);
                query_idx += 1;
            }
        }

        if query_idx == query.len() {
            score
        } else {
            0
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        if !self.active {
            return;
        }

        let area = frame.area();

        // Center the modal
        let modal_width = (area.width as f32 * 0.6).min(80.0) as u16;
        let modal_height = (area.height as f32 * 0.6).min(30.0) as u16;

        let modal_area = Rect {
            x: (area.width - modal_width) / 2,
            y: (area.height - modal_height) / 2,
            width: modal_width,
            height: modal_height,
        };

        // Clear the area behind the modal
        frame.render_widget(Clear, modal_area);

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);
        let dark_bg = Color::Rgb(0x1a, 0x1a, 0x2e);

        // Modal block
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(orange))
            .style(Style::default().bg(dark_bg))
            .title(Line::from(vec![
                Span::styled(" \u{f002} ", Style::default().fg(orange)), // Search icon
                Span::styled("Find Files", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
            ]));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Layout inside modal
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Search input
                Constraint::Length(1), // Tabs
                Constraint::Min(1),    // Results
            ])
            .split(inner);

        // Search input
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .style(Style::default().bg(dark_bg));

        let cursor = "▌";
        let input_text = Line::from(vec![
            Span::styled("\u{f002} ", Style::default().fg(orange)), // Search icon
            Span::raw(&self.query),
            Span::styled(cursor, Style::default().fg(orange)),
        ]);

        let input = Paragraph::new(input_text)
            .block(input_block)
            .style(Style::default().fg(Color::White));

        frame.render_widget(input, layout[0]);

        // Tabs
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
            Span::styled(" \u{f15b} Files ", files_style), // File icon
            Span::raw(" "),
            Span::styled(" \u{f002} Grep ", grep_style), // Search icon
            Span::styled("  (Tab to switch)", Style::default().fg(Color::DarkGray)),
        ]);

        let tabs_widget = Paragraph::new(tabs).style(Style::default().bg(dark_bg));
        frame.render_widget(tabs_widget, layout[1]);

        // Results
        let results_block = Block::default()
            .style(Style::default().bg(dark_bg));

        let items: Vec<ListItem> = self
            .results
            .iter()
            .map(|result| {
                let icon_style = Style::default().fg(result.color);
                let name_style = if result.is_dir {
                    Style::default().fg(orange).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
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
