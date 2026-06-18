//! Review panel: lists code annotations, tracks their status, and dispatches
//! headless Claude Code workers to address them.

use crate::action::Action;
use crate::annotation::{Annotation, AnnotationStatus};
use crate::components::Component;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

/// A worker process addressing a specific annotation.
struct Worker {
    /// Id of the annotation being worked on.
    annotation_id: String,
    /// The spawned `claude` child process.
    child: Child,
    /// When the worker was launched, used to display elapsed time.
    started: Instant,
    /// Display label snapshot (file + lines) for the workers list.
    location: String,
}

/// A finished worker kept in history (hidden unless "show all" is toggled).
struct FinishedWorker {
    /// Display label snapshot (file + lines).
    location: String,
    /// Whether the worker exited successfully.
    success: bool,
    /// How long the worker ran, in seconds.
    elapsed_secs: u64,
}

/// Review panel component.
///
/// Displays all annotations stored under `.cawd/`, lets the user change their
/// status, open the annotated location, and launch a worker on each one.
pub struct Review {
    root: PathBuf,
    annotations: Vec<Annotation>,
    /// Indices into `annotations` that are currently shown (filtered list).
    visible_indices: Vec<usize>,
    list_state: ListState,
    workers: Vec<Worker>,
    /// History of completed workers, shown only when `show_resolved` is on.
    finished: Vec<FinishedWorker>,
    /// When false (default), resolved annotations and finished workers are hidden.
    show_resolved: bool,
    message: Option<String>,
}

impl Review {
    /// Maximum number of finished workers kept in history.
    const FINISHED_HISTORY: usize = 20;

    /// Creates a new review panel and loads existing annotations.
    pub fn new(root: PathBuf) -> Self {
        let mut review = Self {
            root,
            annotations: Vec::new(),
            visible_indices: Vec::new(),
            list_state: ListState::default(),
            workers: Vec::new(),
            finished: Vec::new(),
            show_resolved: false,
            message: None,
        };
        review.refresh();
        review
    }

    /// Builds a short display label (filename + line range) for an annotation.
    fn location_label(annotation: &Annotation) -> String {
        format!(
            "{} L{}",
            annotation.file.rsplit('/').next().unwrap_or(&annotation.file),
            annotation.lines
        )
    }

    /// Recomputes which annotations are visible based on `show_resolved`,
    /// clamping the selection to the visible range.
    fn update_visible(&mut self) {
        self.visible_indices = self
            .annotations
            .iter()
            .enumerate()
            .filter(|(_, a)| self.show_resolved || a.status != AnnotationStatus::Resolved)
            .map(|(i, _)| i)
            .collect();

        if self.visible_indices.is_empty() {
            self.list_state.select(None);
        } else {
            let current = self.list_state.selected().unwrap_or(0);
            self.list_state
                .select(Some(current.min(self.visible_indices.len() - 1)));
        }
    }

    /// Number of resolved annotations currently hidden from the list.
    fn hidden_count(&self) -> usize {
        if self.show_resolved {
            0
        } else {
            self.annotations
                .iter()
                .filter(|a| a.status == AnnotationStatus::Resolved)
                .count()
        }
    }

    /// Toggles visibility of resolved annotations and finished workers.
    fn toggle_show_resolved(&mut self) {
        self.show_resolved = !self.show_resolved;
        self.update_visible();
    }

    /// Reloads annotations from disk, preserving the current selection by id.
    pub fn refresh(&mut self) {
        let selected_id = self.selected().map(|a| a.id.clone());
        self.annotations = Annotation::load_all(&self.root);
        self.update_visible();

        if let Some(id) = selected_id {
            if let Some(pos) = self
                .visible_indices
                .iter()
                .position(|&i| self.annotations[i].id == id)
            {
                self.list_state.select(Some(pos));
            }
        }
    }

    /// Polls running workers and updates annotation status on completion.
    ///
    /// A worker that exits successfully marks its annotation `resolved`; any
    /// other outcome returns it to `open` so it can be retried.
    pub fn poll_workers(&mut self) {
        struct Done {
            index: usize,
            id: String,
            success: bool,
            location: String,
            elapsed: u64,
        }
        let mut done: Vec<Done> = Vec::new();

        for (index, worker) in self.workers.iter_mut().enumerate() {
            let outcome = match worker.child.try_wait() {
                Ok(Some(status)) => Some(status.success()),
                Ok(None) => None,
                Err(_) => Some(false),
            };
            if let Some(success) = outcome {
                done.push(Done {
                    index,
                    id: worker.annotation_id.clone(),
                    success,
                    location: worker.location.clone(),
                    elapsed: worker.started.elapsed().as_secs(),
                });
            }
        }

        if done.is_empty() {
            return;
        }

        for entry in done.into_iter().rev() {
            self.workers.remove(entry.index);
            if let Some(annotation) = self.annotations.iter_mut().find(|a| a.id == entry.id) {
                annotation.status = if entry.success {
                    AnnotationStatus::Resolved
                } else {
                    AnnotationStatus::Open
                };
                annotation.worker_pid = None;
                let _ = annotation.save();
            }
            self.finished.push(FinishedWorker {
                location: entry.location,
                success: entry.success,
                elapsed_secs: entry.elapsed,
            });
        }

        let overflow = self.finished.len().saturating_sub(Self::FINISHED_HISTORY);
        if overflow > 0 {
            self.finished.drain(0..overflow);
        }
        self.update_visible();
    }

    /// Maps the list selection to an index into `annotations`.
    fn real_index(&self) -> Option<usize> {
        let selected = self.list_state.selected()?;
        self.visible_indices.get(selected).copied()
    }

    /// Returns the currently selected annotation, if any.
    fn selected(&self) -> Option<&Annotation> {
        self.real_index().and_then(|i| self.annotations.get(i))
    }

    /// Moves the selection up, wrapping around.
    fn move_up(&mut self) {
        let len = self.visible_indices.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new = if current == 0 { len - 1 } else { current - 1 };
        self.list_state.select(Some(new));
    }

    /// Moves the selection down, wrapping around.
    fn move_down(&mut self) {
        let len = self.visible_indices.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new = if current >= len - 1 { 0 } else { current + 1 };
        self.list_state.select(Some(new));
    }

    /// Returns an action to open the selected annotation in the code viewer.
    fn open_selected(&self) -> Action {
        if let Some(annotation) = self.selected() {
            Action::AnnotationOpen {
                path: self.root.join(&annotation.file),
                line: annotation.start_line,
            }
        } else {
            Action::None
        }
    }

    /// Cycles the status of the selected annotation and persists it.
    fn cycle_status(&mut self) {
        if let Some(i) = self.real_index() {
            if let Some(annotation) = self.annotations.get_mut(i) {
                annotation.status = annotation.status.next();
                let _ = annotation.save();
            }
            self.update_visible();
        }
    }

    /// Deletes the selected annotation from disk and the list.
    fn delete_selected(&mut self) {
        if let Some(i) = self.real_index() {
            if i < self.annotations.len() {
                let annotation = self.annotations.remove(i);
                let _ = annotation.delete();
                self.message = Some(format!("Deleted annotation {}", annotation.id));
                self.update_visible();
            }
        }
    }

    /// Builds the prompt handed to the worker for an annotation.
    fn build_prompt(annotation: &Annotation) -> String {
        format!(
            "A code reviewer left this comment on `{file}` (lines {lines}):\n\n\
             {comment}\n\n\
             The relevant code is:\n\n{excerpt}\n\n\
             Please address this comment by editing the code accordingly.",
            file = annotation.file,
            lines = annotation.lines,
            comment = annotation.comment,
            excerpt = annotation.excerpt,
        )
    }

    /// Launches a headless Claude Code worker on the selected annotation.
    ///
    /// The worker runs `claude -p <prompt> --dangerously-skip-permissions` from
    /// the project root, with output streamed to `.cawd/logs/<id>.log`.
    fn dispatch_worker(&mut self) {
        let Some(index) = self.real_index() else {
            return;
        };
        let Some(annotation) = self.annotations.get(index) else {
            return;
        };
        if annotation.status == AnnotationStatus::InProgress {
            self.message = Some("A worker is already running on this annotation".to_string());
            return;
        }

        let id = annotation.id.clone();
        let location = Self::location_label(annotation);
        let prompt = Self::build_prompt(annotation);

        let log_dir = Annotation::dir(&self.root).join("logs");
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            self.message = Some(format!("Failed to create log dir: {}", e));
            return;
        }

        let (stdout, stderr) = match std::fs::File::create(log_dir.join(format!("{}.log", id))) {
            Ok(file) => match file.try_clone() {
                Ok(clone) => (Stdio::from(file), Stdio::from(clone)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            Err(e) => {
                self.message = Some(format!("Failed to open log file: {}", e));
                return;
            }
        };

        let child = Command::new("claude")
            .arg("-p")
            .arg(&prompt)
            .arg("--dangerously-skip-permissions")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn();

        match child {
            Ok(child) => {
                let pid = child.id();
                if let Some(annotation) = self.annotations.get_mut(index) {
                    annotation.status = AnnotationStatus::InProgress;
                    annotation.worker_pid = Some(pid);
                    let _ = annotation.save();
                }
                self.workers.push(Worker {
                    annotation_id: id,
                    child,
                    started: Instant::now(),
                    location,
                });
                self.message = Some(format!("Worker started (pid {})", pid));
            }
            Err(e) => {
                self.message = Some(format!("Failed to start worker: {}", e));
            }
        }
    }
}

impl Component for Review {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                Action::None
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected(),
            KeyCode::Char('s') => {
                self.cycle_status();
                Action::None
            }
            KeyCode::Char('w') => {
                self.dispatch_worker();
                Action::None
            }
            KeyCode::Char('d') => {
                self.delete_selected();
                Action::None
            }
            KeyCode::Char('a') => {
                self.toggle_show_resolved();
                Action::None
            }
            KeyCode::Char('r') => {
                self.refresh();
                Action::None
            }
            _ => Action::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let chunks = Layout::vertical([
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ])
        .split(area);

        self.render_annotations(frame, chunks[0], focused);
        self.render_workers(frame, chunks[1]);
    }
}

impl Review {
    /// Renders the annotations list (top section of the review panel).
    fn render_annotations(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let resolved = self
            .annotations
            .iter()
            .filter(|a| a.status == AnnotationStatus::Resolved)
            .count();
        let title = if self.show_resolved {
            format!(" \u{f075} Review ({} · all shown) ", self.annotations.len())
        } else if resolved > 0 {
            format!(
                " \u{f075} Review ({} active · {} done) ",
                self.visible_indices.len(),
                resolved
            )
        } else {
            format!(" \u{f075} Review ({}) ", self.visible_indices.len())
        };

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        let hidden = self.hidden_count();
        if let Some(message) = &self.message {
            block = block
                .title_bottom(Line::from(format!(" {} ", message)).style(Style::default().fg(orange)));
        } else if hidden > 0 {
            block = block.title_bottom(
                Line::from(format!(" press 'a' to show {} done ", hidden))
                    .style(Style::default().fg(orange)),
            );
        } else if self.show_resolved {
            block = block.title_bottom(
                Line::from(" press 'a' to hide done ")
                    .style(Style::default().fg(Color::DarkGray)),
            );
        }

        if self.visible_indices.is_empty() {
            let text = if self.annotations.is_empty() {
                " No annotations yet — select code and press 'c' "
            } else {
                " All resolved — press 'a' to show "
            };
            let paragraph = Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem> = self
            .visible_indices
            .iter()
            .filter_map(|&idx| self.annotations.get(idx))
            .map(|annotation| {
                let status_color = match annotation.status {
                    AnnotationStatus::Open => Color::Rgb(0xff, 0xc1, 0x07),
                    AnnotationStatus::InProgress => Color::Rgb(0x2a, 0x9d, 0xf4),
                    AnnotationStatus::Resolved => Color::Rgb(0x28, 0xa7, 0x45),
                };

                let comment_preview: String = annotation.comment.lines().next().unwrap_or("").to_string();
                let comment_preview = if comment_preview.chars().count() > 30 {
                    format!("{}…", comment_preview.chars().take(30).collect::<String>())
                } else {
                    comment_preview
                };

                let line = Line::from(vec![
                    Span::styled(
                        format!(" {} ", annotation.status.glyph()),
                        Style::default().fg(status_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<18} ", Self::location_label(annotation)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(comment_preview, Style::default().fg(Color::DarkGray)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let highlight_style = Style::default()
            .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
            .fg(Color::Rgb(0x1a, 0x12, 0x0f))
            .add_modifier(Modifier::BOLD);

        let list = List::new(items).block(block).highlight_style(highlight_style);

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    /// Renders the workers list (bottom section of the review panel).
    ///
    /// Active workers are always shown; finished workers are listed only when
    /// "show all" (`a`) is toggled on.
    fn render_workers(&self, frame: &mut Frame, area: Rect) {
        let title = if self.show_resolved && !self.finished.is_empty() {
            format!(
                " \u{f085} Workers ({} active · {} done) ",
                self.workers.len(),
                self.finished.len()
            )
        } else {
            format!(" \u{f085} Workers ({}) ", self.workers.len())
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title);

        let show_finished = self.show_resolved && !self.finished.is_empty();

        if self.workers.is_empty() && !show_finished {
            let text = if self.finished.is_empty() {
                " No active workers "
            } else {
                " No active workers — 'a' to show history "
            };
            let paragraph = Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let blue = Color::Rgb(0x2a, 0x9d, 0xf4);
        let green = Color::Rgb(0x28, 0xa7, 0x45);
        let red = Color::Rgb(0xdc, 0x35, 0x45);

        let mut items: Vec<ListItem> = self
            .workers
            .iter()
            .map(|worker| {
                let elapsed = worker.started.elapsed().as_secs();
                ListItem::new(Line::from(vec![
                    Span::styled(" \u{25d0} ", Style::default().fg(blue).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:<18} ", worker.location), Style::default().fg(Color::White)),
                    Span::styled(format!("pid {} ", worker.child.id()), Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{}s", elapsed), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();

        if show_finished {
            for finished in self.finished.iter().rev() {
                let (glyph, color, label) = if finished.success {
                    ("●", green, "done")
                } else {
                    ("✗", red, "failed")
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", glyph), Style::default().fg(color)),
                    Span::styled(format!("{:<18} ", finished.location), Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{} ", label), Style::default().fg(color)),
                    Span::styled(format!("{}s", finished.elapsed_secs), Style::default().fg(Color::DarkGray)),
                ])));
            }
        }

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }
}
