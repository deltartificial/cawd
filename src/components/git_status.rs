use crate::action::Action;
use crate::components::Component;
use crossterm::event::{KeyCode, KeyEvent};
use devicons::FileIcon;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl GitFileStatus {
    fn icon(&self) -> &str {
        match self {
            GitFileStatus::Modified => "M",
            GitFileStatus::Added => "A",
            GitFileStatus::Deleted => "D",
            GitFileStatus::Renamed => "R",
            GitFileStatus::Untracked => "?",
            GitFileStatus::Conflicted => "!",
        }
    }

    fn color(&self) -> Color {
        match self {
            GitFileStatus::Modified => Color::Rgb(0xff, 0xc1, 0x07), // Yellow
            GitFileStatus::Added => Color::Rgb(0x28, 0xa7, 0x45),    // Green
            GitFileStatus::Deleted => Color::Rgb(0xdc, 0x35, 0x45),  // Red
            GitFileStatus::Renamed => Color::Rgb(0x6f, 0x42, 0xc1),  // Purple
            GitFileStatus::Untracked => Color::Rgb(0x6c, 0x75, 0x7d), // Gray
            GitFileStatus::Conflicted => Color::Rgb(0xff, 0x7a, 0x5c), // Orange
        }
    }
}

#[derive(Debug, Clone)]
pub struct GitFile {
    pub path: PathBuf,
    pub name: String,
    pub status: GitFileStatus,
    pub icon: String,
    pub icon_color: Color,
}

impl GitFile {
    pub fn new(path: PathBuf, status: GitFileStatus) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let file_icon = FileIcon::from(&name);
        let icon = file_icon.icon.to_string();
        let icon_color = Self::devicon_color_to_ratatui(&file_icon.color);

        Self {
            path,
            name,
            status,
            icon,
            icon_color,
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

pub struct GitStatus {
    root: PathBuf,
    files: Vec<GitFile>,
    list_state: ListState,
    search_query: String,
    search_mode: bool,
    filtered_indices: Vec<usize>,
}

impl GitStatus {
    pub fn new(root: PathBuf) -> Self {
        let mut status = Self {
            root,
            files: Vec::new(),
            list_state: ListState::default(),
            search_query: String::new(),
            search_mode: false,
            filtered_indices: Vec::new(),
        };
        status.refresh();
        status
    }

    pub fn refresh(&mut self) {
        self.files.clear();

        // Run git status --porcelain
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.root)
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.len() < 3 {
                        continue;
                    }

                    let status_code = &line[0..2];
                    let file_path = line[3..].trim();

                    // Handle renamed files (old -> new)
                    let file_path = if file_path.contains(" -> ") {
                        file_path.split(" -> ").last().unwrap_or(file_path)
                    } else {
                        file_path
                    };

                    let status = Self::parse_status(status_code);
                    let full_path = self.root.join(file_path);
                    self.files.push(GitFile::new(full_path, status));
                }
            }
        }

        // Sort by status priority, then alphabetically
        self.files.sort_by(|a, b| {
            let status_order = |s: &GitFileStatus| match s {
                GitFileStatus::Conflicted => 0,
                GitFileStatus::Modified => 1,
                GitFileStatus::Added => 2,
                GitFileStatus::Deleted => 3,
                GitFileStatus::Renamed => 4,
                GitFileStatus::Untracked => 5,
            };
            match status_order(&a.status).cmp(&status_order(&b.status)) {
                std::cmp::Ordering::Equal => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                other => other,
            }
        });

        self.update_filtered_indices();
        if !self.files.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    fn parse_status(code: &str) -> GitFileStatus {
        let chars: Vec<char> = code.chars().collect();
        let (index, worktree) = (chars.get(0).unwrap_or(&' '), chars.get(1).unwrap_or(&' '));

        // Check for conflicts first
        if *index == 'U' || *worktree == 'U' || (*index == 'A' && *worktree == 'A') || (*index == 'D' && *worktree == 'D') {
            return GitFileStatus::Conflicted;
        }

        // Check staged changes (index)
        match index {
            'M' => return GitFileStatus::Modified,
            'A' => return GitFileStatus::Added,
            'D' => return GitFileStatus::Deleted,
            'R' => return GitFileStatus::Renamed,
            _ => {}
        }

        // Check unstaged changes (worktree)
        match worktree {
            'M' => return GitFileStatus::Modified,
            'D' => return GitFileStatus::Deleted,
            '?' => return GitFileStatus::Untracked,
            _ => {}
        }

        GitFileStatus::Modified
    }

    fn update_filtered_indices(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.files.len()).collect();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_indices = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }
    }

    fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }

    fn selected_file(&self) -> Option<&GitFile> {
        let selected = self.list_state.selected()?;
        let idx = *self.filtered_indices.get(selected)?;
        self.files.get(idx)
    }

    fn move_up(&mut self) {
        if self.visible_count() == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if current == 0 {
            self.visible_count().saturating_sub(1)
        } else {
            current - 1
        };
        self.list_state.select(Some(new_idx));
    }

    fn move_down(&mut self) {
        if self.visible_count() == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if current >= self.visible_count().saturating_sub(1) {
            0
        } else {
            current + 1
        };
        self.list_state.select(Some(new_idx));
    }

    fn select_file(&self) -> Action {
        if let Some(file) = self.selected_file() {
            if file.path.exists() {
                return Action::FileSelected(file.path.clone());
            }
        }
        Action::None
    }

    pub fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.update_filtered_indices();
    }

    pub fn exit_search_mode(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.update_filtered_indices();
        if !self.files.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    pub fn search_input(&mut self, c: char) {
        self.search_query.push(c);
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    pub fn is_search_mode(&self) -> bool {
        self.search_mode
    }
}

impl Component for GitStatus {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.exit_search_mode();
                    Action::ExitSearchMode
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                    self.select_file()
                }
                KeyCode::Backspace => {
                    self.search_backspace();
                    Action::None
                }
                KeyCode::Char(c) => {
                    self.search_input(c);
                    Action::None
                }
                KeyCode::Up | KeyCode::Down => {
                    if key.code == KeyCode::Up {
                        self.move_up();
                    } else {
                        self.move_down();
                    }
                    Action::None
                }
                _ => Action::None,
            }
        } else {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_up();
                    Action::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_down();
                    Action::None
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.select_file(),
                KeyCode::Char('/') => {
                    self.enter_search_mode();
                    Action::EnterSearchMode
                }
                KeyCode::Char('r') => {
                    self.refresh();
                    Action::None
                }
                _ => Action::None,
            }
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let count = self.files.len();
        let title = format!(" \u{f126} Changes ({}) ", count);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        if self.search_mode {
            let search_title = format!(" /{} ", self.search_query);
            block = block.title_bottom(Line::from(search_title).style(Style::default().fg(Color::Yellow)));
        }

        if self.files.is_empty() {
            let empty_text = if focused {
                " No changes "
            } else {
                " No changes "
            };
            let paragraph = ratatui::widgets::Paragraph::new(empty_text)
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem> = self
            .filtered_indices
            .iter()
            .map(|&idx| {
                let file = &self.files[idx];
                let mut spans: Vec<Span> = Vec::new();

                // Status indicator
                spans.push(Span::styled(
                    format!(" {} ", file.status.icon()),
                    Style::default().fg(file.status.color()).add_modifier(Modifier::BOLD),
                ));

                // File icon
                spans.push(Span::styled(
                    format!("{} ", file.icon),
                    Style::default().fg(file.icon_color),
                ));

                // File name
                spans.push(Span::styled(
                    &file.name,
                    Style::default().fg(file.icon_color),
                ));

                ListItem::new(Line::from(spans))
            })
            .collect();

        let highlight_style = Style::default()
            .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
            .fg(Color::Rgb(0x1a, 0x12, 0x0f))
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style);

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }
}
