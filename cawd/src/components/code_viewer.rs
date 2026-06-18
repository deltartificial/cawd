//! Code viewer component with syntax highlighting and search.

use crate::{
    action::Action,
    annotation::{Annotation, AnnotationStatus},
    components::Component,
};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

/// The display mode for the code viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewMode {
    /// Normal syntax-highlighted code view.
    #[default]
    Code,
    /// Git diff view with additions/deletions highlighted.
    Diff,
}
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use syntect::highlighting::FontStyle;

/// Borrowed view of in-file search state passed to the search-modal renderer.
#[derive(Debug)]
struct SearchModalView<'a> {
    /// Current search query text.
    query: &'a str,
    /// Line indices of the current matches.
    matches: &'a [usize],
    /// Index of the highlighted match within `matches`.
    current: usize,
    /// File content, used to render match previews.
    content: &'a [String],
    /// Selection state for the match list.
    list_state: &'a mut ListState,
}

/// Code viewer component with syntax highlighting.
///
/// Displays file contents with syntax-aware highlighting, scrolling,
/// and in-file search with match navigation.
pub(crate) struct CodeViewer {
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
    /// Project root, used to resolve the `.cawd/` annotation directory.
    root: PathBuf,
    /// Inner content area of the last render, used to map mouse rows to lines.
    view_area: Rect,
    /// Start line index of the current selection (inclusive).
    selection_anchor: Option<usize>,
    /// End line index of the current selection (inclusive); moves while dragging.
    selection_cursor: Option<usize>,
    /// Whether a left-button drag selection is in progress.
    is_dragging: bool,
    /// Whether the comment input modal is open.
    comment_mode: bool,
    /// Current text entered in the comment modal.
    comment_input: String,
    /// Transient status message shown after saving an annotation.
    status_message: Option<String>,
    /// Annotations belonging to the currently displayed file (Code view only).
    annotations: Vec<Annotation>,
}

impl CodeViewer {
    /// Creates a new code viewer instance.
    ///
    /// # Parameters
    ///
    /// * `root` - The project root, used to locate the `.cawd/` directory where code annotations
    ///   are saved.
    pub(crate) fn new(root: PathBuf) -> Self {
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
            root,
            view_area: Rect::default(),
            selection_anchor: None,
            selection_cursor: None,
            is_dragging: false,
            comment_mode: false,
            comment_input: String::new(),
            status_message: None,
            annotations: Vec::new(),
        }
    }

    /// Reloads the annotations that belong to the file currently displayed.
    ///
    /// Only populated in [`ViewMode::Code`]; the diff view shows none. Matching
    /// is by the project-relative path stored on each annotation.
    fn reload_annotations(&mut self) {
        self.annotations.clear();
        if self.view_mode != ViewMode::Code {
            return;
        }
        let Some(path) = &self.file_path else {
            return;
        };
        let rel = path
            .strip_prefix(&self.root)
            .map_or(path.as_path(), |it| it)
            .to_string_lossy()
            .into_owned();
        self.annotations =
            Annotation::load_all(&self.root).into_iter().filter(|a| a.file == rel).collect();
    }

    /// Reloads annotations for the open file so status/comment overlays stay in
    /// sync with changes made elsewhere (e.g. the Review panel).
    pub(crate) fn refresh_annotations(&mut self) {
        self.reload_annotations();
    }

    /// Returns the `(background, foreground)` colors used to render an
    /// annotated line range and its inline comment, keyed by status.
    const fn annotation_colors(status: AnnotationStatus) -> (Color, Color) {
        match status {
            AnnotationStatus::Open => (Color::Rgb(0x3a, 0x30, 0x10), Color::Rgb(0xff, 0xc1, 0x07)),
            AnnotationStatus::InProgress => {
                (Color::Rgb(0x10, 0x28, 0x3a), Color::Rgb(0x2a, 0x9d, 0xf4))
            }
            AnnotationStatus::Resolved => {
                (Color::Rgb(0x12, 0x2a, 0x18), Color::Rgb(0x28, 0xa7, 0x45))
            }
        }
    }

    /// Returns the first annotation covering the given 0-based content line.
    fn annotation_at(&self, idx: usize) -> Option<&Annotation> {
        let line = idx + 1; // annotation ranges are 1-based
        self.annotations.iter().find(|a| {
            let (start, end) = a.line_range();
            line >= start && line <= end
        })
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
    pub(crate) fn load_file(&mut self, path: PathBuf) -> color_eyre::Result<()> {
        let content = std::fs::read_to_string(&path)?;
        self.content = content.lines().map(String::from).collect();
        self.file_path = Some(path.clone());
        self.scroll_offset = 0;
        self.search_query.clear();
        self.search_matches.clear();
        self.view_mode = ViewMode::Code;
        self.clear_selection();
        self.status_message = None;

        self.highlight_content(&path);
        self.reload_annotations();

        Ok(())
    }

    /// Displays an error message in the viewer.
    ///
    /// # Parameters
    ///
    /// * `message` - The error message to display.
    pub(crate) fn show_error(&mut self, message: &str) {
        self.content = vec![String::new(), format!("  Error: {}", message), String::new()];
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
    pub(crate) fn load_diff(&mut self, path: PathBuf) -> color_eyre::Result<()> {
        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .arg(&path)
            .current_dir(path.parent().map_or(path.as_path(), |it| it))
            .output()?;

        let diff_content = String::from_utf8_lossy(&output.stdout);

        if diff_content.is_empty() {
            let staged_output = Command::new("git")
                .args(["diff", "--cached"])
                .arg(&path)
                .current_dir(path.parent().map_or(path.as_path(), |it| it))
                .output()?;

            let staged_diff = String::from_utf8_lossy(&staged_output.stdout);
            if staged_diff.is_empty() {
                self.content = vec!["No changes to display".to_owned()];
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
        self.clear_selection();
        self.status_message = None;

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
                !line.starts_with("diff ") &&
                    !line.starts_with("index ") &&
                    !line.starts_with("---") &&
                    !line.starts_with("+++") &&
                    !line.starts_with("@@") &&
                    !line.starts_with('\\')
            })
            .collect();

        let mut line_num = 0usize;
        self.highlighted_lines = filtered
            .iter()
            .map(|line| {
                line_num += 1;
                let num_str = format!("{line_num:>4} │ ");
                let content = if line.len() > 1 { &line[1..] } else { "" };

                if line.starts_with('+') {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(green_fg).bg(green_bg)),
                        Span::styled(
                            format!("+ {content}"),
                            Style::default().fg(green_fg).bg(green_bg),
                        ),
                    ])
                } else if line.starts_with('-') {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(red_fg).bg(red_bg)),
                        Span::styled(
                            format!("- {content}"),
                            Style::default().fg(red_fg).bg(red_bg),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(num_str, Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("  {content}"), Style::default().fg(Color::White)),
                    ])
                }
            })
            .collect();

        self.content = filtered;
    }

    /// Applies syntax highlighting to the loaded content.
    fn highlight_content(&mut self, path: &Path) {
        let extension = path.extension().and_then(|e| e.to_str()).map_or("txt", |it| it);

        let mapped_extension = match extension {
            "sol" | "cairo" | "move" => "rs",
            "vyper" | "vy" => "py",
            ext => ext,
        };

        let syntax = self
            .syntax_set
            .find_syntax_by_extension(mapped_extension)
            .or_else(|| self.syntax_set.find_syntax_by_extension(extension))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let Some(theme) = self.theme_set.themes.get("base16-ocean.dark") else {
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
                        Span::raw(line.clone()),
                    ])
                })
                .collect();
            return;
        };

        let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

        self.highlighted_lines = self
            .content
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let line_num = format!("{:>4} │ ", idx + 1);
                let mut spans: Vec<Span<'static>> =
                    vec![Span::styled(line_num, Style::default().fg(Color::DarkGray))];

                if let Ok(highlighted) = highlighter.highlight_line(line, &self.syntax_set) {
                    for (style, text) in highlighted {
                        let fg =
                            Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);

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

                        spans.push(Span::styled(text.to_owned(), ratatui_style));
                    }
                } else {
                    spans.push(Span::raw(line.clone()));
                }

                Line::from(spans)
            })
            .collect();
    }

    /// Scrolls the view up by the specified amount.
    const fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scrolls the view down by the specified amount.
    fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.content.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    /// Scrolls to the top of the file.
    const fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scrolls to the bottom of the file.
    const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.content.len().saturating_sub(1);
    }

    /// Clears the current line selection.
    const fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_cursor = None;
        self.is_dragging = false;
    }

    /// Returns the selected line range as `(start, end)` indices, ordered.
    ///
    /// Returns `None` when no selection is active.
    fn selected_range(&self) -> Option<(usize, usize)> {
        match (self.selection_anchor, self.selection_cursor) {
            (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
            _ => None,
        }
    }

    /// Maps a terminal row to a content line index within the current view.
    ///
    /// Returns `None` when the row is outside the rendered content area or
    /// beyond the end of the file.
    fn row_to_line(&self, row: u16) -> Option<usize> {
        let top = self.view_area.y;
        let bottom = self.view_area.y.saturating_add(self.view_area.height);
        if row < top || row >= bottom {
            return None;
        }
        let line = self.scroll_offset + (row - top) as usize;
        (line < self.content.len()).then_some(line)
    }

    /// Returns whether a point lies within the rendered content area.
    const fn area_contains(&self, column: u16, row: u16) -> bool {
        let a = self.view_area;
        column >= a.x &&
            column < a.x.saturating_add(a.width) &&
            row >= a.y &&
            row < a.y.saturating_add(a.height)
    }

    /// Handles a mouse event, returning whether it was consumed.
    ///
    /// Left click-and-drag selects a range of lines; the scroll wheel scrolls
    /// the view. Selection is only available in [`ViewMode::Code`].
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode/MouseEventKind are non_exhaustive, a catch-all arm is required"
    )]
    pub(crate) fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        if self.file_path.is_none() {
            return false;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.view_mode != ViewMode::Code {
                    return self.area_contains(mouse.column, mouse.row);
                }
                if let Some(line) = self.row_to_line(mouse.row) {
                    self.status_message = None;
                    self.selection_anchor = Some(line);
                    self.selection_cursor = Some(line);
                    self.is_dragging = true;
                    return true;
                }
                self.area_contains(mouse.column, mouse.row)
            }
            MouseEventKind::Drag(MouseButton::Left) if self.is_dragging => {
                // Auto-scroll when dragging past the top/bottom edge.
                if mouse.row < self.view_area.y {
                    self.scroll_up(1);
                } else if mouse.row >= self.view_area.y.saturating_add(self.view_area.height) {
                    self.scroll_down(1);
                }
                let clamped_row = mouse.row.clamp(
                    self.view_area.y,
                    self.view_area.y.saturating_add(self.view_area.height).saturating_sub(1),
                );
                if let Some(line) = self.row_to_line(clamped_row) {
                    self.selection_cursor = Some(line);
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.is_dragging => {
                self.is_dragging = false;
                true
            }
            MouseEventKind::ScrollDown if self.area_contains(mouse.column, mouse.row) => {
                self.scroll_down(3);
                true
            }
            MouseEventKind::ScrollUp if self.area_contains(mouse.column, mouse.row) => {
                self.scroll_up(3);
                true
            }
            _ => false,
        }
    }

    /// Returns whether the comment input modal is currently open.
    pub(crate) const fn is_commenting(&self) -> bool {
        self.comment_mode
    }

    /// Opens the comment modal for the current selection, if any.
    fn enter_comment_mode(&mut self) {
        if self.view_mode == ViewMode::Code && self.selected_range().is_some() {
            self.comment_mode = true;
            self.comment_input.clear();
            self.status_message = None;
        }
    }

    /// Closes the comment modal without saving.
    fn cancel_comment(&mut self) {
        self.comment_mode = false;
        self.comment_input.clear();
    }

    /// Saves the current selection and comment as an annotation file.
    ///
    /// Writes a timestamped markdown file under `<root>/.cawd/` containing the
    /// file path, line range, the selected code excerpt, and the comment.
    fn save_annotation(&mut self) {
        let Some((start, end)) = self.selected_range() else {
            self.cancel_comment();
            return;
        };
        let Some(file_path) = self.file_path.clone() else {
            self.cancel_comment();
            return;
        };

        let now = chrono::Local::now();
        let id = now.format("%Y-%m-%dT%H-%M-%S").to_string();

        let rel_path = file_path
            .strip_prefix(&self.root)
            .map_or(file_path.as_path(), |it| it)
            .to_string_lossy()
            .into_owned();

        let line_label = if start == end {
            format!("{}", start + 1)
        } else {
            format!("{}-{}", start + 1, end + 1)
        };

        let excerpt: String = self
            .content
            .get(start..=end.min(self.content.len().saturating_sub(1)))
            .map_or(&[][..], |it| it)
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {}", start + 1 + i, line))
            .collect::<Vec<_>>()
            .join("\n");

        let annotation = Annotation {
            id: id.clone(),
            status: AnnotationStatus::Open,
            file: rel_path,
            lines: line_label,
            start_line: start + 1,
            date: now.format("%Y-%m-%d %H:%M:%S").to_string(),
            worker_pid: None,
            excerpt,
            comment: self.comment_input.trim_end().to_owned(),
            path: Annotation::dir(&self.root).join(format!("{id}.md")),
        };

        self.status_message = Some(match annotation.save() {
            Ok(()) => format!("Annotation saved to .cawd/{id}.md"),
            Err(e) => format!("Failed to save annotation: {e}"),
        });

        self.comment_mode = false;
        self.comment_input.clear();
        self.clear_selection();
        self.reload_annotations();
    }

    /// Scrolls the view so the given 1-based line is at the top.
    pub(crate) fn scroll_to_line(&mut self, line: usize) {
        let target = line.saturating_sub(1);
        self.scroll_offset = target.min(self.content.len().saturating_sub(1));
    }

    /// Enters search mode.
    fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_match = 0;
    }

    /// Exits search mode.
    const fn exit_search_mode(&mut self) {
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
        self.current_match =
            self.current_match.checked_sub(1).unwrap_or_else(|| len.saturating_sub(1));
        if let Some(&offset) = self.search_matches.get(self.current_match) {
            self.scroll_offset = offset;
        }
        self.search_list_state.select(Some(self.current_match));
    }

    /// Returns whether the component is in search mode.
    pub(crate) const fn is_search_mode(&self) -> bool {
        self.search_mode
    }

    /// Returns whether a file is currently loaded.
    pub(crate) const fn has_file(&self) -> bool {
        self.file_path.is_some()
    }

    /// Returns the name of the currently loaded file, if any.
    pub(crate) fn file_name(&self) -> Option<&str> {
        self.file_path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str())
    }

    /// Renders the welcome screen with animated logo.
    fn render_welcome(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        self.animation_frame = self.animation_frame.wrapping_add(1);

        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block =
            Block::default().borders(Borders::ALL).border_style(border_style).title(" Welcome ");

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
            let phase = (i as f64).mul_add(0.6, frame_offset);
            let wave = f64::midpoint(phase.sin(), 1.0);

            let r = (f64::from(0xaau8) + f64::from(0xff - 0xaa) * wave) as u8;
            let g = (f64::from(0x44u8) + f64::from(0x7a - 0x44) * wave) as u8;
            let b = (f64::from(0x22u8) + f64::from(0x5c - 0x22) * wave) as u8;

            let color = Color::Rgb(r, g, b);
            logo.push(Line::from(Span::styled(
                (*line_text).to_owned(),
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
        logo.push(Line::from(vec![
            Span::styled("  drag  ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Select lines with mouse", Style::default().fg(Color::White)),
        ]));
        logo.push(Line::from(vec![
            Span::styled("  c     ", Style::default().fg(dark_orange).add_modifier(Modifier::BOLD)),
            Span::styled("Comment selected lines", Style::default().fg(Color::White)),
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
    fn render_search_modal_static(frame: &mut Frame<'_>, area: Rect, view: SearchModalView<'_>) {
        let SearchModalView {
            query: search_query,
            matches: search_matches,
            current: current_match,
            content,
            list_state: search_list_state,
        } = view;
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);
        let dark_bg = Color::Rgb(0x1a, 0x1a, 0x2e);
        let dark_text = Color::Rgb(0x1a, 0x12, 0x0f);

        let modal_width = (f32::from(area.width) * 0.7).min(70.0) as u16;
        let modal_height = (f32::from(area.height) * 0.5).min(20.0) as u16;

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
                Span::styled(
                    "Search in File",
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(inner);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .style(Style::default().bg(dark_bg));

        let cursor = "▌";
        let match_info = if search_matches.is_empty() {
            if search_query.is_empty() { String::new() } else { " (no matches)".to_owned() }
        } else {
            format!(" ({}/{})", current_match + 1, search_matches.len())
        };

        let input_text = Line::from(vec![
            Span::styled("\u{f002} ", Style::default().fg(orange)),
            Span::raw(search_query),
            Span::styled(cursor, Style::default().fg(orange)),
            Span::styled(match_info, Style::default().fg(Color::DarkGray)),
        ]);

        let input =
            Paragraph::new(input_text).block(input_block).style(Style::default().fg(Color::White));

        frame.render_widget(input, layout[0]);

        let items: Vec<ListItem<'_>> = search_matches
            .iter()
            .take(15)
            .map(|&line_idx| {
                let line_num = format!("{:>4}", line_idx + 1);
                let line_content = content.get(line_idx).map(|s| s.trim()).unwrap_or_default();
                let truncated: String = if line_content.chars().count() > 50 {
                    format!("{}...", line_content.chars().take(50).collect::<String>())
                } else {
                    line_content.to_owned()
                };

                let line = Line::from(vec![
                    Span::styled(format!("{line_num} "), Style::default().fg(orange)),
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(truncated, Style::default().fg(Color::White)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let highlight_style =
            Style::default().bg(dark_orange).fg(dark_text).add_modifier(Modifier::BOLD);

        let results_block = Block::default().style(Style::default().bg(dark_bg));

        let list = List::new(items)
            .block(results_block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, layout[1], search_list_state);
    }

    /// Renders the comment input modal for the current line selection.
    fn render_comment_modal_static(
        frame: &mut Frame<'_>,
        area: Rect,
        start: usize,
        end: usize,
        comment_input: &str,
    ) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_bg = Color::Rgb(0x1a, 0x1a, 0x2e);

        let modal_width = (f32::from(area.width) * 0.7).min(70.0) as u16;
        let modal_height = 7u16.min(area.height);

        let modal_area = Rect {
            x: area.x + (area.width.saturating_sub(modal_width)) / 2,
            y: area.y + (area.height.saturating_sub(modal_height)) / 2,
            width: modal_width,
            height: modal_height,
        };

        frame.render_widget(Clear, modal_area);

        let range_label = if start == end {
            format!(" Comment on L{} ", start + 1)
        } else {
            format!(" Comment on L{}-L{} ", start + 1, end + 1)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(orange))
            .style(Style::default().bg(dark_bg))
            .title(Line::from(vec![
                Span::styled(" \u{f075} ", Style::default().fg(orange)),
                Span::styled(
                    range_label,
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]))
            .title_bottom(
                Line::from(" Enter: save │ Esc: cancel ")
                    .style(Style::default().fg(Color::DarkGray)),
            );

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let text = Line::from(vec![
            Span::raw(comment_input),
            Span::styled("▌", Style::default().fg(orange)),
        ]);

        let input = Paragraph::new(text)
            .style(Style::default().fg(Color::White))
            .wrap(ratatui::widgets::Wrap { trim: false });

        frame.render_widget(input, inner);
    }
}

impl Component for CodeViewer {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode/MouseEventKind are non_exhaustive, a catch-all arm is required"
    )]
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.comment_mode {
            match key.code {
                KeyCode::Esc => self.cancel_comment(),
                KeyCode::Enter => self.save_annotation(),
                KeyCode::Backspace => {
                    self.comment_input.pop();
                }
                KeyCode::Char(c) => self.comment_input.push(c),
                _ => {}
            }
            Action::None
        } else if self.search_mode {
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
                KeyCode::Char('c') => {
                    self.enter_comment_mode();
                    Action::None
                }
                KeyCode::Esc => {
                    self.clear_selection();
                    self.status_message = None;
                    Action::None
                }
                _ => Action::None,
            }
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
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
            .map_or_else(
                || " Code ".to_owned(),
                |n| {
                    if self.view_mode == ViewMode::Diff {
                        format!(" [DIFF] {n} ")
                    } else {
                        format!(" {n} ")
                    }
                },
            );

        let mut block =
            Block::default().borders(Borders::ALL).border_style(border_style).title(title);

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let selection = self.selected_range();

        if !self.search_mode && !self.search_query.is_empty() && !self.search_matches.is_empty() {
            let match_info =
                format!(" {}/{} matches ", self.current_match + 1, self.search_matches.len());
            block = block.title_bottom(Line::from(match_info).style(Style::default().fg(orange)));
        } else if let Some((start, end)) = selection {
            let count = end - start + 1;
            let info = format!(" {count} line(s) selected — c: comment ");
            block = block.title_bottom(
                Line::from(info).style(Style::default().fg(orange).add_modifier(Modifier::BOLD)),
            );
        } else if let Some(msg) = &self.status_message {
            block = block
                .title_bottom(Line::from(format!(" {msg} ")).style(Style::default().fg(orange)));
        } else if !self.annotations.is_empty() {
            let label = if self.annotations.len() == 1 {
                " 1 annotation ".to_owned()
            } else {
                format!(" {} annotations ", self.annotations.len())
            };
            block = block.title_bottom(Line::from(label).style(Style::default().fg(orange)));
        }

        self.view_area = block.inner(area);

        let highlight_style =
            Style::default().bg(Color::Rgb(0xe6, 0x5a, 0x3d)).fg(Color::Rgb(0x1a, 0x12, 0x0f));
        let selection_style = Style::default().bg(Color::Rgb(0x3a, 0x4a, 0x6a));

        let visible_lines: Vec<Line<'_>> = self
            .highlighted_lines
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .map(|(idx, line)| {
                if self.search_matches.contains(&idx) {
                    Line::from(vec![Span::styled(line.to_string(), highlight_style)])
                } else if selection.is_some_and(|(s, e)| idx >= s && idx <= e) {
                    line.clone().patch_style(selection_style)
                } else if let Some(annotation) = self.annotation_at(idx) {
                    let (bg, fg) = Self::annotation_colors(annotation.status);
                    let mut annotated = line.clone().patch_style(Style::default().bg(bg));
                    // Show the comment inline on the first line of the range.
                    if idx + 1 == annotation.line_range().0 {
                        let comment = annotation.comment.lines().next().unwrap_or_default();
                        annotated.spans.push(Span::styled(
                            format!("   {} {}", annotation.status.glyph(), comment),
                            Style::default().fg(fg).add_modifier(Modifier::BOLD | Modifier::ITALIC),
                        ));
                    }
                    annotated
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
                SearchModalView {
                    query: &self.search_query,
                    matches: &self.search_matches,
                    current: self.current_match,
                    content: &self.content,
                    list_state: &mut self.search_list_state,
                },
            );
        }

        if self.comment_mode &&
            let Some((start, end)) = selection
        {
            Self::render_comment_modal_static(frame, area, start, end, &self.comment_input);
        }
    }
}
