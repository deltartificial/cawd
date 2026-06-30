//! Git status component showing changed files in the repository.

use crate::{action::Action, components::Component};
use crossterm::event::{KeyCode, KeyEvent};
use devicons::FileIcon;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};
use std::{path::PathBuf, process::Command};

/// Represents the git status of a file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GitFileStatus {
    /// File has been modified.
    Modified,
    /// File has been added to staging.
    Added,
    /// File has been deleted.
    Deleted,
    /// File has been renamed.
    Renamed,
    /// File is not tracked by git.
    Untracked,
    /// File has merge conflicts.
    Conflicted,
}

impl GitFileStatus {
    /// Returns the single-character status indicator.
    const fn icon(&self) -> &str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
            Self::Conflicted => "!",
        }
    }

    /// Returns the color associated with this status.
    const fn color(&self) -> Color {
        match self {
            Self::Modified => Color::Rgb(0xff, 0xc1, 0x07),
            Self::Added => Color::Rgb(0x28, 0xa7, 0x45),
            Self::Deleted => Color::Rgb(0xdc, 0x35, 0x45),
            Self::Renamed => Color::Rgb(0x6f, 0x42, 0xc1),
            Self::Untracked => Color::Rgb(0x6c, 0x75, 0x7d),
            Self::Conflicted => Color::Rgb(0xff, 0x7a, 0x5c),
        }
    }
}

/// Represents a file with git changes.
#[derive(Debug, Clone)]
pub(crate) struct GitFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Display name (filename only).
    pub name: String,
    /// The git status of the file.
    pub status: GitFileStatus,
    /// File type icon.
    pub icon: String,
    /// Color for the file icon.
    pub icon_color: Color,
}

impl GitFile {
    /// Creates a new `GitFile` from a path and status.
    ///
    /// # Parameters
    ///
    /// * `path` - The absolute path to the file.
    /// * `status` - The git status of the file.
    pub(crate) fn new(path: PathBuf, status: GitFileStatus) -> Self {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        let file_icon = FileIcon::from(&name);
        let icon = file_icon.icon.to_string();
        let icon_color = Self::devicon_color_to_ratatui(file_icon.color);

        Self { path, name, status, icon, icon_color }
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

/// A single entry in the recent-commits card.
#[derive(Debug, Clone)]
struct CommitInfo {
    /// Abbreviated commit hash.
    hash: String,
    /// First line of the commit message.
    summary: String,
}

/// Which card inside the Changes panel currently holds the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ChangesFocus {
    /// The changed-files list (top card).
    #[default]
    Files,
    /// The recent-commits list (bottom card).
    Commits,
}

/// Git status panel component.
///
/// Displays a list of files with uncommitted changes, supporting
/// navigation, filtering, and file selection.
pub(crate) struct GitStatus {
    root: PathBuf,
    files: Vec<GitFile>,
    list_state: ListState,
    search_query: String,
    search_mode: bool,
    filtered_indices: Vec<usize>,
    commits: Vec<CommitInfo>,
    commits_state: ListState,
    focus: ChangesFocus,
}

impl GitStatus {
    /// Creates a new `GitStatus` component.
    ///
    /// # Parameters
    ///
    /// * `root` - The root directory of the git repository.
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut status = Self {
            root,
            files: Vec::new(),
            list_state: ListState::default(),
            search_query: String::new(),
            search_mode: false,
            filtered_indices: Vec::new(),
            commits: Vec::new(),
            commits_state: ListState::default(),
            focus: ChangesFocus::Files,
        };
        status.refresh();
        status
    }

    /// Reloads the most recent commits shown in the commits card.
    fn refresh_commits(&mut self) {
        self.commits.clear();
        let log_output = Command::new("git")
            .args(["log", "--pretty=format:%h%x1f%s", "-n", "40"])
            .current_dir(&self.root)
            .output();

        if let Ok(output) = log_output &&
            output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let mut parts = line.split('\u{1f}');
                let (Some(hash), Some(summary)) = (parts.next(), parts.next()) else {
                    continue;
                };
                self.commits
                    .push(CommitInfo { hash: hash.to_owned(), summary: summary.to_owned() });
            }
        }
    }

    /// Refreshes the list of changed files from git.
    ///
    /// Runs `git status --porcelain` and parses the output.
    /// Preserves the current selection if the file still exists.
    pub(crate) fn refresh(&mut self) {
        let selected_path = self.selected_file().map(|f| f.path.clone());

        self.files.clear();

        let git_output =
            Command::new("git").args(["status", "--porcelain"]).current_dir(&self.root).output();

        if let Ok(output) = git_output &&
            output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let Some(status_code) = line.get(0..2) else { continue };
                let Some(raw_path) = line.get(3..) else { continue };
                let trimmed = raw_path.trim();

                let file_path = trimmed.split(" -> ").last().map_or(trimmed, |it| it);

                let status = Self::parse_status(status_code);
                let full_path = self.root.join(file_path);
                self.files.push(GitFile::new(full_path, status));
            }
        }

        self.files.sort_by(|a, b| {
            let status_order = |s: &GitFileStatus| match s {
                GitFileStatus::Conflicted => 0,
                GitFileStatus::Modified => 1,
                GitFileStatus::Added => 2,
                GitFileStatus::Deleted => 3,
                GitFileStatus::Renamed => 4,
                GitFileStatus::Untracked => 5,
            };
            status_order(&a.status)
                .cmp(&status_order(&b.status))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        self.update_filtered_indices();
        self.refresh_commits();

        if let Some(path) = selected_path &&
            let Some(pos) = self.files.iter().position(|f| f.path == path) &&
            let Some(filtered_pos) = self.filtered_indices.iter().position(|&i| i == pos)
        {
            self.list_state.select(Some(filtered_pos));
            return;
        }

        if !self.files.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Parses a git status code into a `GitFileStatus`.
    fn parse_status(code: &str) -> GitFileStatus {
        let chars: Vec<char> = code.chars().collect();
        let (index, worktree) =
            (chars.first().map_or(&' ', |it| it), chars.get(1).map_or(&' ', |it| it));

        if *index == 'U' ||
            *worktree == 'U' ||
            (*index == 'A' && *worktree == 'A') ||
            (*index == 'D' && *worktree == 'D')
        {
            return GitFileStatus::Conflicted;
        }

        match index {
            'M' => return GitFileStatus::Modified,
            'A' => return GitFileStatus::Added,
            'D' => return GitFileStatus::Deleted,
            'R' => return GitFileStatus::Renamed,
            _ => {}
        }

        match worktree {
            'M' => return GitFileStatus::Modified,
            'D' => return GitFileStatus::Deleted,
            '?' => return GitFileStatus::Untracked,
            _ => {}
        }

        GitFileStatus::Modified
    }

    /// Updates the filtered indices based on the current search query.
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

    /// Returns the number of visible items after filtering.
    const fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }

    /// Returns the currently selected file, if any.
    fn selected_file(&self) -> Option<&GitFile> {
        let selected = self.list_state.selected()?;
        let idx = *self.filtered_indices.get(selected)?;
        self.files.get(idx)
    }

    /// Moves the cursor up, crossing from commits back into files at the top.
    fn move_up(&mut self) {
        match self.focus {
            ChangesFocus::Files => {
                if self.visible_count() == 0 {
                    return;
                }
                let current = self.list_state.selected().unwrap_or_default();
                if current > 0 {
                    self.list_state.select(Some(current - 1));
                }
            }
            ChangesFocus::Commits => {
                let current = self.commits_state.selected().unwrap_or_default();
                if current > 0 {
                    self.commits_state.select(Some(current - 1));
                } else if self.visible_count() > 0 {
                    self.focus = ChangesFocus::Files;
                    self.list_state.select(Some(self.visible_count() - 1));
                }
            }
        }
    }

    /// Moves the cursor down, crossing from files into commits at the bottom.
    fn move_down(&mut self) {
        match self.focus {
            ChangesFocus::Files => {
                let count = self.visible_count();
                let current = self.list_state.selected().unwrap_or_default();
                if count > 0 && current + 1 < count {
                    self.list_state.select(Some(current + 1));
                } else if !self.search_mode && !self.commits.is_empty() {
                    self.focus = ChangesFocus::Commits;
                    self.commits_state.select(Some(0));
                }
            }
            ChangesFocus::Commits => {
                if self.commits.is_empty() {
                    return;
                }
                let current = self.commits_state.selected().unwrap_or_default();
                if current + 1 < self.commits.len() {
                    self.commits_state.select(Some(current + 1));
                }
            }
        }
    }

    /// Opens the focused item: a file diff, or a commit's full diff.
    fn select(&self) -> Action {
        match self.focus {
            ChangesFocus::Files => {
                if let Some(file) = self.selected_file() {
                    return Action::DiffSelected(file.path.clone());
                }
                Action::None
            }
            ChangesFocus::Commits => {
                if let Some(sel) = self.commits_state.selected() &&
                    let Some(commit) = self.commits.get(sel)
                {
                    return Action::CommitSelected(commit.hash.clone());
                }
                Action::None
            }
        }
    }

    /// Enters search/filter mode.
    pub(crate) fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.focus = ChangesFocus::Files;
        self.update_filtered_indices();
    }

    /// Exits search/filter mode and clears the query.
    pub(crate) fn exit_search_mode(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.update_filtered_indices();
        if !self.files.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Appends a character to the search query.
    pub(crate) fn search_input(&mut self, c: char) {
        self.search_query.push(c);
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    /// Removes the last character from the search query.
    pub(crate) fn search_backspace(&mut self) {
        self.search_query.pop();
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    /// Returns whether the component is in search mode.
    pub(crate) const fn is_search_mode(&self) -> bool {
        self.search_mode
    }
}

impl Component for GitStatus {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode/MouseEventKind are non_exhaustive, a catch-all arm is required"
    )]
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.exit_search_mode();
                    Action::ExitSearchMode
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                    self.select()
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
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.select(),
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

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let border_style = if focused && self.focus == ChangesFocus::Files {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let count = self.files.len();
        let title = format!(" \u{f126} Changes ({count}) ");

        let mut block =
            Block::default().borders(Borders::ALL).border_style(border_style).title(title);

        if self.search_mode {
            let search_title = format!(" /{} ", self.search_query);
            block = block
                .title_bottom(Line::from(search_title).style(Style::default().fg(Color::Yellow)));
        }

        if self.files.is_empty() {
            let paragraph = ratatui::widgets::Paragraph::new(" No changes ")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem<'_>> = self
            .filtered_indices
            .iter()
            .filter_map(|&idx| self.files.get(idx))
            .map(|file| {
                let mut spans: Vec<Span<'_>> = Vec::with_capacity(3);

                spans.push(Span::styled(
                    format!(" {} ", file.status.icon()),
                    Style::default().fg(file.status.color()).add_modifier(Modifier::BOLD),
                ));

                spans.push(Span::styled(
                    format!("{} ", file.icon),
                    Style::default().fg(file.icon_color),
                ));

                spans.push(Span::styled(&file.name, Style::default().fg(file.icon_color)));

                ListItem::new(Line::from(spans))
            })
            .collect();

        let highlight_style = Style::default()
            .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
            .fg(Color::Rgb(0x1a, 0x12, 0x0f))
            .add_modifier(Modifier::BOLD);

        let list = List::new(items).block(block).highlight_style(highlight_style);

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }
}

impl GitStatus {
    /// Renders the recent-commits card, shown below the changed files when the
    /// Changes panel has the left column to itself.
    pub(crate) fn render_commits(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let active = self.focus == ChangesFocus::Commits;
        let border_style = if active {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" \u{f417} Commits ({}) ", self.commits.len()));

        if self.commits.is_empty() {
            let paragraph = ratatui::widgets::Paragraph::new(" No commits ")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem<'_>> = self
            .commits
            .iter()
            .map(|commit| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", commit.hash), Style::default().fg(orange)),
                    Span::styled(&commit.summary, Style::default().fg(Color::Gray)),
                ]))
            })
            .collect();

        let highlight_style = Style::default()
            .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
            .fg(Color::Rgb(0x1a, 0x12, 0x0f))
            .add_modifier(Modifier::BOLD);

        let list = List::new(items).block(block).highlight_style(highlight_style);
        frame.render_stateful_widget(list, area, &mut self.commits_state);
    }
}
