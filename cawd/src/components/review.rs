//! Review panel: lists code annotations, tracks their status, and dispatches
//! headless Claude Code workers to address them.

use crate::{
    action::Action,
    annotation::{Annotation, AnnotationStatus},
    components::Component,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Instant,
};

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
    /// Commit+push job to enqueue once the worker finishes successfully.
    ///
    /// `None` for plain workers (those just edit in place). When set, the job
    /// is appended to the serialized commit queue rather than run inline, so
    /// parallel workers never race on the git index or clobber each other's
    /// push.
    commit_job: Option<CommitJob>,
}

/// A pending commit+push for a finished worker.
///
/// These run one at a time (see [`Review::commit_in_flight`]) so that ten
/// workers finishing at once still commit and push in a clean sequence.
#[derive(Clone)]
struct CommitJob {
    /// Id of the annotation this commit resolves.
    id: String,
    /// Project-relative path of the single file to stage and commit.
    file: String,
    /// The commit message.
    message: String,
    /// Display label (file + lines) for the workers list.
    location: String,
}

/// The commit+push currently executing (at most one at a time).
struct CommitInFlight {
    /// Id of the annotation being committed.
    id: String,
    /// Display label for the workers list.
    location: String,
    /// The spawned `git` shell pipeline.
    child: Child,
    /// When the commit started, for elapsed display.
    started: Instant,
}

/// A finished worker kept in history (hidden unless "show all" is toggled).
struct FinishedWorker {
    /// Display label snapshot (file + lines).
    location: String,
    /// Whether the worker exited successfully.
    success: bool,
    /// How long the worker ran, in seconds.
    elapsed_secs: u64,
    /// Whether this worker was set to commit and push its changes.
    commit: bool,
}

/// Review panel component.
///
/// Displays all annotations stored under `.cawd/`, lets the user change their
/// status, open the annotated location, and launch a worker on each one.
pub(crate) struct Review {
    root: PathBuf,
    annotations: Vec<Annotation>,
    /// Indices into `annotations` that are currently shown (filtered list).
    visible_indices: Vec<usize>,
    list_state: ListState,
    workers: Vec<Worker>,
    /// Commit+push jobs waiting their turn, processed one at a time.
    commit_queue: Vec<CommitJob>,
    /// The commit+push currently running, if any.
    commit_in_flight: Option<CommitInFlight>,
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
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut review = Self {
            root,
            annotations: Vec::new(),
            visible_indices: Vec::new(),
            list_state: ListState::default(),
            workers: Vec::new(),
            commit_queue: Vec::new(),
            commit_in_flight: None,
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
            annotation.file.rsplit('/').next().map_or(annotation.file.as_str(), |it| it),
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
            let current = self.list_state.selected().unwrap_or_default();
            self.list_state.select(Some(current.min(self.visible_indices.len() - 1)));
        }
    }

    /// Number of resolved annotations currently hidden from the list.
    fn hidden_count(&self) -> usize {
        if self.show_resolved {
            0
        } else {
            self.annotations.iter().filter(|a| a.status == AnnotationStatus::Resolved).count()
        }
    }

    /// Toggles visibility of resolved annotations and finished workers.
    fn toggle_show_resolved(&mut self) {
        self.show_resolved = !self.show_resolved;
        self.update_visible();
    }

    /// Reloads annotations from disk, preserving the current selection by id.
    pub(crate) fn refresh(&mut self) {
        let selected_id = self.selected().map(|a| a.id.clone());
        self.annotations = Annotation::load_all(&self.root);
        self.update_visible();

        if let Some(id) = selected_id &&
            let Some(pos) =
                self.visible_indices.iter().position(|&i| self.annotations[i].id == id)
        {
            self.list_state.select(Some(pos));
        }
    }

    /// Sets an annotation's status, clears its worker pid, and persists it.
    fn set_annotation_status(&mut self, id: &str, status: AnnotationStatus) {
        if let Some(annotation) = self.annotations.iter_mut().find(|a| a.id == id) {
            annotation.status = status;
            annotation.worker_pid = None;
            _ = annotation.save();
        }
    }

    /// Advances all background work: reaps finished edit-phase workers, reaps
    /// the in-flight commit+push, and starts the next queued commit if idle.
    ///
    /// A plain worker that exits successfully marks its annotation `resolved`;
    /// a committing one instead enqueues a serialized commit+push and stays
    /// `in_progress` until that pushes. Any failure returns the annotation to
    /// `open` so it can be retried.
    pub(crate) fn poll_workers(&mut self) {
        let mut changed = self.reap_workers();
        changed |= self.reap_commit();
        // Kick off the next queued commit whenever the single git slot is free.
        if self.commit_in_flight.is_none() &&
            let Some(job) = (!self.commit_queue.is_empty()).then(|| self.commit_queue.remove(0))
        {
            self.start_commit(job);
            changed = true;
        }

        if changed {
            let overflow = self.finished.len().saturating_sub(Self::FINISHED_HISTORY);
            if overflow > 0 {
                self.finished.drain(0..overflow);
            }
            self.update_visible();
        }
    }

    /// Reaps edit-phase workers that have exited. Returns whether any did.
    fn reap_workers(&mut self) -> bool {
        struct Done {
            index: usize,
            id: String,
            success: bool,
            location: String,
            elapsed: u64,
            commit_job: Option<CommitJob>,
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
                    commit_job: worker.commit_job.clone(),
                });
            }
        }

        if done.is_empty() {
            return false;
        }

        for entry in done.into_iter().rev() {
            self.workers.remove(entry.index);
            match (entry.success, entry.commit_job) {
                (true, Some(job)) => {
                    // Edit succeeded: drop the worker pid but keep the
                    // annotation in progress until its commit+push runs.
                    self.set_annotation_status(&entry.id, AnnotationStatus::InProgress);
                    self.commit_queue.push(job);
                }
                (true, None) => {
                    self.set_annotation_status(&entry.id, AnnotationStatus::Resolved);
                    self.finished.push(FinishedWorker {
                        location: entry.location,
                        success: true,
                        elapsed_secs: entry.elapsed,
                        commit: false,
                    });
                }
                (false, commit_job) => {
                    self.set_annotation_status(&entry.id, AnnotationStatus::Open);
                    self.finished.push(FinishedWorker {
                        location: entry.location,
                        success: false,
                        elapsed_secs: entry.elapsed,
                        commit: commit_job.is_some(),
                    });
                }
            }
        }
        true
    }

    /// Reaps the in-flight commit+push, if it has finished. Returns whether it
    /// did, recording the outcome and resolving (or reopening) the annotation.
    fn reap_commit(&mut self) -> bool {
        let Some(commit) = &mut self.commit_in_flight else {
            return false;
        };
        let outcome = match commit.child.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        };
        let Some(success) = outcome else {
            return false;
        };

        let id = commit.id.clone();
        let location = commit.location.clone();
        let elapsed = commit.started.elapsed().as_secs();
        self.commit_in_flight = None;

        let status = if success { AnnotationStatus::Resolved } else { AnnotationStatus::Open };
        self.set_annotation_status(&id, status);
        self.finished.push(FinishedWorker {
            location,
            success,
            elapsed_secs: elapsed,
            commit: true,
        });
        true
    }

    /// Spawns the serialized commit+push for a queued job.
    ///
    /// Runs in the repo root and stages, commits, then pushes only the job's
    /// file. `git commit -- <file>` keeps the commit scoped to that one path
    /// regardless of what else sits in the index. The push retries once behind
    /// `git pull --rebase` in case the upstream moved. On spawn failure the
    /// annotation is reopened. File and message travel via environment
    /// variables so neither needs shell-escaping.
    fn start_commit(&mut self, job: CommitJob) {
        let log_dir = Annotation::dir(&self.root).join("logs");
        _ = std::fs::create_dir_all(&log_dir);

        // Prefer the subject the worker authored over the comment-derived
        // fallback: the former describes the change, the latter the complaint.
        // Take the first non-empty line so a stray trailing newline or body the
        // worker may add can't leak into the subject.
        let msg_path = self.root.join(Self::commit_msg_rel_path(&job.id));
        let generated = std::fs::read_to_string(&msg_path)
            .ok()
            .and_then(|raw| raw.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned));
        _ = std::fs::remove_file(&msg_path);
        let message = match generated {
            Some(subject) => subject,
            None => job.message,
        };
        let (stdout, stderr) =
            match std::fs::File::create(log_dir.join(format!("{}-commit.log", job.id))) {
                Ok(file) => match file.try_clone() {
                    Ok(clone) => (Stdio::from(file), Stdio::from(clone)),
                    Err(_) => (Stdio::null(), Stdio::null()),
                },
                Err(_) => (Stdio::null(), Stdio::null()),
            };

        let spawn_result = Command::new("sh")
            .arg("-c")
            .arg(
                "git add -- \"$CAWD_FILE\" \
                 && git commit -m \"$CAWD_COMMIT_MSG\" -- \"$CAWD_FILE\" \
                 && { git push || { git pull --rebase && git push; }; }",
            )
            .env("CAWD_FILE", &job.file)
            .env("CAWD_COMMIT_MSG", &message)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn();

        match spawn_result {
            Ok(child) => {
                self.commit_in_flight = Some(CommitInFlight {
                    id: job.id,
                    location: job.location,
                    child,
                    started: Instant::now(),
                });
            }
            Err(e) => {
                self.set_annotation_status(&job.id, AnnotationStatus::Open);
                self.message = Some(format!("Failed to start commit: {e}"));
            }
        }
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
        let current = self.list_state.selected().unwrap_or_default();
        let new = if current == 0 { len - 1 } else { current - 1 };
        self.list_state.select(Some(new));
    }

    /// Moves the selection down, wrapping around.
    fn move_down(&mut self) {
        let len = self.visible_indices.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or_default();
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
                _ = annotation.save();
            }
            self.update_visible();
        }
    }

    /// Deletes the selected annotation from disk and the list.
    fn delete_selected(&mut self) {
        if let Some(i) = self.real_index() &&
            i < self.annotations.len()
        {
            let annotation = self.annotations.remove(i);
            _ = annotation.delete();
            self.message = Some(format!("Deleted annotation {}", annotation.id));
            self.update_visible();
        }
    }

    /// Builds the prompt handed to the worker for an annotation.
    ///
    /// When `commit` is set, cawd commits and pushes the changes itself once the
    /// worker exits, so the worker is told to edit only and leave git alone.
    fn build_prompt(annotation: &Annotation, commit: bool) -> String {
        let git_note = if commit {
            format!(
                "\n\nDo not run git, commit, or push: only edit the files. \
                 cawd will commit and push the result for you. \
                 After editing, write a single-line Conventional Commits subject \
                 (e.g. `fix(scope): description`) that describes the change you \
                 actually made — not the review comment — to the file `{path}`. \
                 Keep it under 72 characters and write nothing else to that file.",
                path = Self::commit_msg_rel_path(&annotation.id),
            )
        } else {
            String::new()
        };
        format!(
            "A code reviewer left this comment on `{file}` (lines {lines}):\n\n\
             {comment}\n\n\
             The relevant code is:\n\n{excerpt}\n\n\
             Please address this comment by editing the code accordingly.{git_note}",
            file = annotation.file,
            lines = annotation.lines,
            comment = annotation.comment,
            excerpt = annotation.excerpt,
        )
    }

    /// Project-relative path where a committing worker writes the Conventional
    /// Commits subject it generates for its annotation. Relative because the
    /// worker runs with the project root as its working directory.
    fn commit_msg_rel_path(id: &str) -> String {
        format!(".cawd/logs/{id}.commitmsg")
    }

    /// Fallback commit message, used only when the worker did not write a
    /// generated subject. Derived from the reviewer's comment, which describes
    /// the problem rather than the fix, so it reads poorly — hence the worker is
    /// asked to author a proper subject (see [`Self::build_prompt`]).
    fn build_commit_message(annotation: &Annotation) -> String {
        let summary = annotation.comment.lines().next().map_or("address review comment", str::trim);
        format!("fix(review): {summary} ({} L{})", annotation.file, annotation.lines)
    }

    /// Launches a headless Claude Code worker on the selected annotation.
    ///
    /// When `commit` is set, the worker also commits and pushes its changes
    /// once it finishes successfully.
    fn dispatch_worker(&mut self, commit: bool) {
        let Some(index) = self.real_index() else {
            return;
        };
        self.dispatch_worker_at(index, commit);
    }

    /// Dispatches a worker on the annotation with the given id, if present.
    ///
    /// Used when a worker is requested straight from the code viewer's comment
    /// dialog (Ctrl+W / Ctrl+G), right after the annotation has been saved.
    pub(crate) fn dispatch_worker_for_id(&mut self, id: &str, commit: bool) {
        if let Some(index) = self.annotations.iter().position(|a| a.id == id) {
            self.dispatch_worker_at(index, commit);
        }
    }

    /// Builds the headless worker command:
    /// `claude -p <prompt> --dangerously-skip-permissions`.
    ///
    /// The prompt is a direct argument (no shell), so it needs no escaping.
    /// Committing workers do not run git here: their commit+push is queued and
    /// run serially elsewhere (see [`Self::start_commit`]) so parallel workers
    /// never race on the index or clobber each other's push.
    fn worker_command(root: &Path, prompt: &str) -> Command {
        let mut command = Command::new("claude");
        command.arg("-p").arg(prompt).arg("--dangerously-skip-permissions").current_dir(root);
        command
    }

    /// Launches a worker on the annotation at `index` in `annotations`.
    fn dispatch_worker_at(&mut self, index: usize, commit: bool) {
        let Some(annotation) = self.annotations.get(index) else {
            return;
        };
        if annotation.status == AnnotationStatus::InProgress {
            self.message = Some("A worker is already running on this annotation".to_owned());
            return;
        }

        let id = annotation.id.clone();
        let location = Self::location_label(annotation);
        let prompt = Self::build_prompt(annotation, commit);
        // A committing worker enqueues a serialized commit+push on success,
        // staging only this annotation's file.
        let commit_job = commit.then(|| CommitJob {
            id: id.clone(),
            file: annotation.file.clone(),
            message: Self::build_commit_message(annotation),
            location: location.clone(),
        });

        let log_dir = Annotation::dir(&self.root).join("logs");
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            self.message = Some(format!("Failed to create log dir: {e}"));
            return;
        }

        let (stdout, stderr) = match std::fs::File::create(log_dir.join(format!("{id}.log"))) {
            Ok(file) => match file.try_clone() {
                Ok(clone) => (Stdio::from(file), Stdio::from(clone)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            Err(e) => {
                self.message = Some(format!("Failed to open log file: {e}"));
                return;
            }
        };

        let spawn_result = Self::worker_command(&self.root, &prompt)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn();

        match spawn_result {
            Ok(child) => {
                let pid = child.id();
                if let Some(annotation) = self.annotations.get_mut(index) {
                    annotation.status = AnnotationStatus::InProgress;
                    annotation.worker_pid = Some(pid);
                    _ = annotation.save();
                }
                self.message = Some(if commit {
                    format!("Worker started (pid {pid}); commits + pushes when done")
                } else {
                    format!("Worker started (pid {pid})")
                });
                self.workers.push(Worker {
                    annotation_id: id,
                    child,
                    started: Instant::now(),
                    location,
                    commit_job,
                });
            }
            Err(e) => {
                self.message = Some(format!("Failed to start worker: {e}"));
            }
        }
    }
}

impl Component for Review {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode/MouseEventKind are non_exhaustive, a catch-all arm is required"
    )]
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
                self.dispatch_worker(false);
                Action::None
            }
            KeyCode::Char('W') => {
                self.dispatch_worker(true);
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

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let chunks =
            Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)]).split(area);

        self.render_annotations(frame, chunks[0], focused);
        self.render_workers(frame, chunks[1]);
    }
}

impl Review {
    /// Renders the annotations list (top section of the review panel).
    fn render_annotations(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let resolved =
            self.annotations.iter().filter(|a| a.status == AnnotationStatus::Resolved).count();
        let title = if self.show_resolved {
            format!(" \u{f075} Review ({} · all shown) ", self.annotations.len())
        } else if resolved > 0 {
            format!(" \u{f075} Review ({} active · {} done) ", self.visible_indices.len(), resolved)
        } else {
            format!(" \u{f075} Review ({}) ", self.visible_indices.len())
        };

        let block = Block::default().borders(Borders::ALL).border_style(border_style).title(title);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Reserve the last inner row for an always-present hint line, drawn
        // inside the panel just above the bottom border.
        let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
        let list_area = rows[0];
        let hint_area = rows[1];

        let hidden = self.hidden_count();
        let hint_line = if let Some(message) = &self.message {
            Line::from(format!(" {message} ")).style(Style::default().fg(orange))
        } else {
            let label = if self.show_resolved {
                "hide done".to_owned()
            } else if hidden > 0 {
                format!("show {hidden} done")
            } else {
                "show done".to_owned()
            };
            Line::from(vec![
                Span::styled(" a ", Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(label, Style::default().fg(Color::DarkGray)),
            ])
        };
        frame.render_widget(Paragraph::new(hint_line), hint_area);

        if self.visible_indices.is_empty() {
            let text = if self.annotations.is_empty() {
                " No annotations yet — select code and press 'c' "
            } else {
                " All resolved — press 'a' to show "
            };
            let paragraph = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, list_area);
            return;
        }

        let items: Vec<ListItem<'_>> = self
            .visible_indices
            .iter()
            .filter_map(|&idx| self.annotations.get(idx))
            .map(|annotation| {
                let status_color = match annotation.status {
                    AnnotationStatus::Open => Color::Rgb(0xff, 0xc1, 0x07),
                    AnnotationStatus::InProgress => Color::Rgb(0x2a, 0x9d, 0xf4),
                    AnnotationStatus::Resolved => Color::Rgb(0x28, 0xa7, 0x45),
                };

                let first_line = annotation.comment.lines().next().unwrap_or_default();
                let comment_preview = if first_line.chars().count() > 30 {
                    format!("{}…", first_line.chars().take(30).collect::<String>())
                } else {
                    first_line.to_owned()
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

        let list = List::new(items).highlight_style(highlight_style);

        frame.render_stateful_widget(list, list_area, &mut self.list_state);
    }

    /// Renders the workers list (bottom section of the review panel).
    ///
    /// Active workers are always shown; finished workers are listed only when
    /// "show all" (`a`) is toggled on.
    fn render_workers(&self, frame: &mut Frame<'_>, area: Rect) {
        // Anything past the edit phase: the running commit plus those queued.
        let pending = self.commit_queue.len() + usize::from(self.commit_in_flight.is_some());
        let active = self.workers.len();
        let title = if self.show_resolved && !self.finished.is_empty() {
            format!(" \u{f085} Workers ({active} active · {} done) ", self.finished.len())
        } else if pending > 0 {
            format!(" \u{f085} Workers ({active} active · {pending} to push) ")
        } else {
            format!(" \u{f085} Workers ({active}) ")
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title);

        let show_finished = self.show_resolved && !self.finished.is_empty();

        if self.workers.is_empty() && pending == 0 && !show_finished {
            let text = if self.finished.is_empty() {
                " No active workers "
            } else {
                " No active workers — 'a' to show history "
            };
            let paragraph =
                Paragraph::new(text).block(block).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }

        let blue = Color::Rgb(0x2a, 0x9d, 0xf4);
        let green = Color::Rgb(0x28, 0xa7, 0x45);
        let red = Color::Rgb(0xdc, 0x35, 0x45);

        let mut items: Vec<ListItem<'_>> = self
            .workers
            .iter()
            .map(|worker| {
                let elapsed = worker.started.elapsed().as_secs();
                let ship = if worker.commit_job.is_some() { "\u{21e1} " } else { "" };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        " \u{25d0} ",
                        Style::default().fg(blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<18} ", worker.location),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(ship, Style::default().fg(blue).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!("pid {} ", worker.child.id()),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(format!("{elapsed}s"), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();

        // The commit+push currently running, then those waiting their turn.
        if let Some(commit) = &self.commit_in_flight {
            let elapsed = commit.started.elapsed().as_secs();
            items.push(ListItem::new(Line::from(vec![
                Span::styled(" \u{21e1} ", Style::default().fg(green).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{:<18} ", commit.location),
                    Style::default().fg(Color::White),
                ),
                Span::styled("pushing ", Style::default().fg(green)),
                Span::styled(format!("{elapsed}s"), Style::default().fg(Color::DarkGray)),
            ])));
        }
        for job in &self.commit_queue {
            items.push(ListItem::new(Line::from(vec![
                Span::styled(" \u{2026} ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:<18} ", job.location), Style::default().fg(Color::Gray)),
                Span::styled("queued", Style::default().fg(Color::DarkGray)),
            ])));
        }

        if show_finished {
            for finished in self.finished.iter().rev() {
                let (glyph, color, base_label) =
                    if finished.success { ("●", green, "done") } else { ("✗", red, "failed") };
                let label = if finished.commit && finished.success {
                    "pushed".to_owned()
                } else {
                    base_label.to_owned()
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!(" {glyph} "), Style::default().fg(color)),
                    Span::styled(
                        format!("{:<18} ", finished.location),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(format!("{label} "), Style::default().fg(color)),
                    Span::styled(
                        format!("{}s", finished.elapsed_secs),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])));
            }
        }

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }
}
