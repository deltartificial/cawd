//! Notion panel: a read-only view of tickets pulled from a Notion page.
//!
//! Network access lives entirely on a background worker thread that owns a
//! Tokio runtime and a `reqwest` client. The UI loop never blocks on the
//! network: it pushes refresh requests and drains results through `mpsc`
//! channels, mirroring how the rest of the app stays responsive.

use crate::{action::Action, components::Component};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Child, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

/// Notion REST API base URL.
const API_BASE: &str = "https://api.notion.com/v1";

/// Pinned Notion API version sent on every request.
const NOTION_VERSION: &str = "2022-06-28";

/// Default source id (dashless) used when `NOTION_PAGE_ID` is unset: the
/// "TASK RUN TECH" database. Works whether the id is a page wrapping a database
/// or the database itself (see `fetch_tickets`).
const DEFAULT_PAGE_ID: &str = "3813b9213ea780c7b718fb7681d0904e";

/// How often the panel auto-refreshes while focused.
const AUTO_REFRESH: Duration = Duration::from_secs(60);

/// A single ticket surfaced from the Notion page.
#[derive(Debug, Clone)]
struct Ticket {
    /// Notion page id of the ticket (used to lazily fetch its body).
    id: String,
    /// Ticket title (the Notion `title` property, or page name).
    title: String,
    /// Names of the people assigned to the ticket, if any.
    assignees: Vec<String>,
    /// Status/select label, when the source database exposes one.
    status: Option<String>,
    /// Canonical Notion URL, opened in the browser on `Enter`.
    url: String,
    /// Remaining properties rendered as `(name, value)` for the detail pane.
    fields: Vec<(String, String)>,
}

impl Ticket {
    /// Whether at least one person is assigned to this ticket.
    const fn assigned(&self) -> bool {
        !self.assignees.is_empty()
    }
}

/// A request sent to the background worker thread.
#[derive(Debug)]
enum Command {
    /// Re-fetch the whole ticket list.
    Refresh,
    /// Fetch the body (child blocks) of a single ticket page by id.
    Detail(String),
}

/// Outcome of a background fetch, delivered back to the UI thread.
#[derive(Debug)]
enum FetchResult {
    /// The ticket list was (re)fetched.
    List(Result<Vec<Ticket>, String>),
    /// A ticket's body was fetched; carries the ticket id and rendered lines.
    Detail {
        /// Notion page id this body belongs to.
        id: String,
        /// Rendered body lines, or an error message.
        result: Result<Vec<String>, String>,
    },
}

/// Loading state of a single ticket's body in the detail pane.
#[derive(Debug, Clone)]
enum DetailState {
    /// The body fetch is in flight.
    Loading,
    /// The body was fetched; carries the rendered lines.
    Ready(Vec<String>),
    /// The body fetch failed; carries the error message.
    Failed(String),
}

/// A headless `claude` worker advancing a single ticket.
#[derive(Debug)]
struct Worker {
    /// Notion id of the ticket the worker is addressing.
    ticket_id: String,
    /// The spawned `claude` child process.
    child: Child,
    /// When the worker was launched, used to display elapsed time.
    started: Instant,
    /// Short title snapshot for the workers list.
    title: String,
}

/// A finished worker kept in history.
#[derive(Debug)]
struct FinishedWorker {
    /// Notion id of the ticket the worker addressed (for opening its log).
    ticket_id: String,
    /// Short title snapshot.
    title: String,
    /// Whether the worker exited successfully.
    success: bool,
    /// How long the worker ran, in seconds.
    elapsed_secs: u64,
}

/// Which of the panel's three sub-panes currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// The ticket list (top-left).
    List,
    /// The ticket detail (right): scrollable content.
    Detail,
    /// The workers pane (bottom-left): selectable worker logs.
    Workers,
}

/// High-level loading state shown in the panel header.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// `NOTION_TOKEN` is not set; the panel explains how to configure it.
    NoToken,
    /// A fetch is in flight (or the very first one is pending).
    Loading,
    /// At least one fetch has completed successfully.
    Ready,
    /// The last fetch failed; carries the error message.
    Failed(String),
}

/// Read-only Notion tickets panel.
#[derive(Debug)]
pub(crate) struct Notion {
    /// Repository root; workers run here and logs land under `.cawd/logs`.
    root: PathBuf,
    /// All tickets from the last successful fetch.
    tickets: Vec<Ticket>,
    /// Indices into `tickets` currently visible after filter/search.
    filtered_indices: Vec<usize>,
    /// Selection cursor over `filtered_indices`.
    list_state: ListState,
    /// Current loading/error state.
    status: Status,
    /// When false (default), only assigned tickets are shown.
    show_unassigned: bool,
    /// Active title filter query.
    search_query: String,
    /// Whether the panel is in incremental-search mode.
    search_mode: bool,
    /// Cached ticket bodies, keyed by Notion page id, for the detail pane.
    details: BTreeMap<String, DetailState>,
    /// Vertical scroll offset of the detail pane.
    detail_scroll: u16,
    /// Which sub-pane currently holds focus.
    focus: Focus,
    /// Currently running ticket workers.
    workers: Vec<Worker>,
    /// History of completed workers (most recent last).
    finished: Vec<FinishedWorker>,
    /// Selection cursor over the workers pane (active then finished).
    worker_state: ListState,
    /// Transient status line shown in the workers pane.
    message: Option<String>,
    /// Sends commands to the worker; `None` when there is no token.
    request_tx: Option<Sender<Command>>,
    /// Receives fetch results from the worker; `None` when there is no token.
    result_rx: Option<Receiver<FetchResult>>,
    /// Whether a refresh request is currently outstanding.
    in_flight: bool,
    /// Last time an auto-refresh fired.
    last_refresh: Instant,
}

impl Notion {
    /// Maximum number of finished workers kept in history.
    const FINISHED_HISTORY: usize = 12;

    /// Creates the panel, reading `NOTION_TOKEN` (and optional `NOTION_PAGE_ID`)
    /// from the environment and spawning the network worker when a token exists.
    ///
    /// `root` is the repository the dispatched workers operate in.
    pub(crate) fn new(root: PathBuf) -> Self {
        let token = env_string("NOTION_TOKEN").filter(|t| !t.trim().is_empty());
        let page_id = env_string("NOTION_PAGE_ID")
            .map_or_else(|| DEFAULT_PAGE_ID.to_owned(), |raw| normalize_page_id(&raw));

        let (status, request_tx, result_rx) = match token {
            Some(tok) => {
                let (req_tx, res_rx) = spawn_worker(tok, page_id);
                // Kick off the first fetch immediately.
                let status = if req_tx.send(Command::Refresh).is_ok() {
                    Status::Loading
                } else {
                    Status::Failed("worker unavailable".to_owned())
                };
                (status, Some(req_tx), Some(res_rx))
            }
            None => (Status::NoToken, None, None),
        };

        let in_flight = matches!(status, Status::Loading);

        Self {
            root,
            tickets: Vec::new(),
            filtered_indices: Vec::new(),
            list_state: ListState::default(),
            status,
            show_unassigned: false,
            search_query: String::new(),
            search_mode: false,
            details: BTreeMap::new(),
            detail_scroll: 0,
            focus: Focus::List,
            workers: Vec::new(),
            finished: Vec::new(),
            worker_state: ListState::default(),
            message: None,
            request_tx,
            result_rx,
            in_flight,
            last_refresh: Instant::now(),
        }
    }

    /// Requests a fresh fetch from the worker, unless one is already running.
    pub(crate) fn refresh(&mut self) {
        if self.in_flight {
            return;
        }
        if let Some(tx) = self.request_tx.as_ref() &&
            tx.send(Command::Refresh).is_ok()
        {
            self.in_flight = true;
            self.status = Status::Loading;
            self.last_refresh = Instant::now();
        }
    }

    /// Drains all completed fetch results and runs the periodic auto-refresh.
    ///
    /// Called from the app's idle tick so the UI thread never blocks.
    pub(crate) fn poll(&mut self) {
        let mut messages = Vec::new();
        if let Some(rx) = self.result_rx.as_ref() {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }

        for message in messages {
            match message {
                FetchResult::List(Ok(mut tickets)) => {
                    self.in_flight = false;
                    sort_tickets(&mut tickets);
                    self.tickets = tickets;
                    // Bodies may have changed; drop the cache and re-request the
                    // selected ticket's body via `update_filtered_indices`.
                    self.details.clear();
                    self.status = Status::Ready;
                    self.update_filtered_indices();
                }
                FetchResult::List(Err(e)) => {
                    self.in_flight = false;
                    self.status = Status::Failed(e);
                }
                FetchResult::Detail { id, result } => {
                    let state = match result {
                        Ok(lines) => DetailState::Ready(lines),
                        Err(e) => DetailState::Failed(e),
                    };
                    self.details.insert(id, state);
                }
            }
        }

        if !self.in_flight && self.last_refresh.elapsed() >= AUTO_REFRESH {
            self.refresh();
        }

        self.poll_workers();
    }

    /// Reaps finished ticket workers, moving them into the history list.
    fn poll_workers(&mut self) {
        struct Done {
            index: usize,
            ticket_id: String,
            title: String,
            success: bool,
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
                    ticket_id: worker.ticket_id.clone(),
                    title: worker.title.clone(),
                    success,
                    elapsed: worker.started.elapsed().as_secs(),
                });
            }
        }

        if done.is_empty() {
            return;
        }

        for entry in done.into_iter().rev() {
            self.workers.remove(entry.index);
            self.finished.push(FinishedWorker {
                ticket_id: entry.ticket_id,
                title: entry.title,
                success: entry.success,
                elapsed_secs: entry.elapsed,
            });
        }

        let overflow = self.finished.len().saturating_sub(Self::FINISHED_HISTORY);
        if overflow > 0 {
            self.finished.drain(0..overflow);
        }
        self.clamp_worker_selection();
    }

    /// Total number of rows shown in the workers pane.
    const fn worker_count(&self) -> usize {
        self.workers.len() + self.finished.len()
    }

    /// Keeps the workers selection within range as the lists change.
    fn clamp_worker_selection(&mut self) {
        let count = self.worker_count();
        if count == 0 {
            self.worker_state.select(None);
        } else if let Some(sel) = self.worker_state.selected() {
            self.worker_state.select(Some(sel.min(count - 1)));
        }
    }

    /// Builds the log path for a ticket's worker output.
    fn worker_log_path(&self, ticket_id: &str) -> PathBuf {
        let slug: String = ticket_id.chars().filter(char::is_ascii_alphanumeric).collect();
        self.root.join(".cawd").join("logs").join(format!("notion-{slug}.log"))
    }

    /// Worker ids in display order: active workers, then finished (newest first).
    fn ordered_worker_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.workers.iter().map(|w| w.ticket_id.as_str()).collect();
        ids.extend(self.finished.iter().rev().map(|f| f.ticket_id.as_str()));
        ids
    }

    /// Opens the selected worker's log file in the code viewer.
    fn open_selected_worker_log(&self) -> Action {
        let ids = self.ordered_worker_ids();
        let Some(sel) = self.worker_state.selected() else {
            return Action::None;
        };
        match ids.get(sel) {
            Some(id) => Action::FileSelected(self.worker_log_path(id)),
            None => Action::None,
        }
    }

    /// Moves the workers selection up one row.
    fn worker_up(&mut self) {
        if self.worker_count() == 0 {
            return;
        }
        let current = self.worker_state.selected().unwrap_or_default();
        if current > 0 {
            self.worker_state.select(Some(current - 1));
        }
    }

    /// Moves the workers selection down one row.
    fn worker_down(&mut self) {
        let count = self.worker_count();
        if count == 0 {
            return;
        }
        let current = self.worker_state.selected().unwrap_or_default();
        if current + 1 < count {
            self.worker_state.select(Some(current + 1));
        }
    }

    /// Sets the focused sub-pane, selecting the first worker when entering it.
    fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        if focus == Focus::Workers &&
            self.worker_state.selected().is_none() &&
            self.worker_count() > 0
        {
            self.worker_state.select(Some(0));
        }
    }

    /// Resets focus to the ticket list. Called when the panel gains focus.
    pub(crate) const fn focus_list(&mut self) {
        self.focus = Focus::List;
    }

    /// Advances focus to the next sub-pane, wrapping around within the panel.
    pub(crate) fn focus_next(&mut self) {
        match self.focus {
            Focus::List => self.set_focus(Focus::Detail),
            Focus::Detail => self.set_focus(Focus::Workers),
            Focus::Workers => self.set_focus(Focus::List),
        }
    }

    /// Moves focus to the previous sub-pane, wrapping around within the panel.
    pub(crate) fn focus_prev(&mut self) {
        match self.focus {
            Focus::List => self.set_focus(Focus::Workers),
            Focus::Detail => self.set_focus(Focus::List),
            Focus::Workers => self.set_focus(Focus::Detail),
        }
    }

    /// Launches a headless `claude` worker on the selected ticket.
    ///
    /// The worker runs `claude -p <prompt> --dangerously-skip-permissions` from
    /// the repository root, with output streamed to `.cawd/logs/notion-<id>.log`.
    /// Notion is read-only, so worker state is tracked only in memory.
    fn dispatch_worker(&mut self) {
        let Some(ticket) = self.selected_ticket() else {
            return;
        };
        if ticket.id.is_empty() {
            self.message = Some("This ticket has no page id to work on".to_owned());
            return;
        }
        if self.workers.iter().any(|w| w.ticket_id == ticket.id) {
            self.message = Some("A worker is already running on this ticket".to_owned());
            return;
        }

        let ticket_id = ticket.id.clone();
        let title = ticket.title.clone();
        let body = match self.details.get(&ticket_id) {
            Some(DetailState::Ready(lines)) => lines.clone(),
            _ => Vec::new(),
        };
        let prompt = build_worker_prompt(ticket, &body);

        let log_path = self.worker_log_path(&ticket_id);
        if let Some(dir) = log_path.parent() &&
            let Err(e) = std::fs::create_dir_all(dir)
        {
            self.message = Some(format!("Failed to create log dir: {e}"));
            return;
        }

        let (stdout, stderr) = match std::fs::File::create(&log_path) {
            Ok(file) => match file.try_clone() {
                Ok(clone) => (Stdio::from(file), Stdio::from(clone)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            Err(e) => {
                self.message = Some(format!("Failed to open log file: {e}"));
                return;
            }
        };

        let spawn_result = std::process::Command::new("claude")
            .arg("-p")
            .arg(&prompt)
            .arg("--dangerously-skip-permissions")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn();

        match spawn_result {
            Ok(child) => {
                let pid = child.id();
                self.workers.push(Worker { ticket_id, child, started: Instant::now(), title });
                self.message = Some(format!("Worker started (pid {pid})"));
            }
            Err(e) => {
                self.message =
                    Some(format!("Failed to start worker: {e} (is `claude` installed?)"));
            }
        }
    }

    /// Resets the detail scroll and requests the selected ticket's body.
    fn after_selection_change(&mut self) {
        self.detail_scroll = 0;
        let Some(id) = self.selected_ticket().map(|t| t.id.clone()) else {
            return;
        };
        if id.is_empty() || self.details.contains_key(&id) {
            return;
        }
        if let Some(tx) = self.request_tx.as_ref() &&
            tx.send(Command::Detail(id.clone())).is_ok()
        {
            self.details.insert(id, DetailState::Loading);
        }
    }

    /// Returns whether the panel is in incremental-search mode.
    pub(crate) const fn is_search_mode(&self) -> bool {
        self.search_mode
    }

    /// Recomputes `filtered_indices` from the assignment filter and query.
    fn update_filtered_indices(&mut self) {
        let query = self.search_query.to_lowercase();
        self.filtered_indices = self
            .tickets
            .iter()
            .enumerate()
            .filter(|(_, t)| self.show_unassigned || t.assigned())
            .filter(|(_, t)| query.is_empty() || t.title.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            let max = self.filtered_indices.len() - 1;
            let sel = self.list_state.selected().unwrap_or_default().min(max);
            self.list_state.select(Some(sel));
        }
        self.after_selection_change();
    }

    /// Moves the selection cursor up one row.
    fn move_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or_default();
        if current > 0 {
            self.list_state.select(Some(current - 1));
            self.after_selection_change();
        }
    }

    /// Moves the selection cursor down one row.
    fn move_down(&mut self) {
        let count = self.filtered_indices.len();
        if count == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or_default();
        if current + 1 < count {
            self.list_state.select(Some(current + 1));
            self.after_selection_change();
        }
    }

    /// Returns the currently selected ticket, if any.
    fn selected_ticket(&self) -> Option<&Ticket> {
        let sel = self.list_state.selected()?;
        let idx = *self.filtered_indices.get(sel)?;
        self.tickets.get(idx)
    }

    /// Opens the selected ticket's Notion URL in the default browser.
    fn open_selected(&self) -> Action {
        match self.selected_ticket() {
            Some(t) if !t.url.is_empty() => Action::OpenUrl(t.url.clone()),
            _ => Action::None,
        }
    }

    /// Toggles whether unassigned tickets are shown.
    fn toggle_unassigned(&mut self) {
        self.show_unassigned = !self.show_unassigned;
        self.update_filtered_indices();
    }

    /// Enters incremental-search mode.
    fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.update_filtered_indices();
    }

    /// Exits incremental-search mode, clearing the query.
    fn exit_search_mode(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.update_filtered_indices();
    }
}

/// Sorts tickets: assigned first, then by status label, then by title.
fn sort_tickets(tickets: &mut [Ticket]) {
    tickets.sort_by(|a, b| {
        b.assigned()
            .cmp(&a.assigned())
            .then_with(|| a.status.cmp(&b.status))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Reads an environment variable as a UTF-8 string, when set and valid.
fn env_string(key: &str) -> Option<String> {
    std::env::var_os(key).and_then(|v| v.into_string().ok())
}

/// Normalizes a raw page id or Notion URL into a dashless 32-char hex id.
fn normalize_page_id(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() >= 32 {
        hex.chars().skip(hex.len() - 32).collect()
    } else if hex.is_empty() {
        DEFAULT_PAGE_ID.to_owned()
    } else {
        hex
    }
}

impl Component for Notion {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode is non_exhaustive, a catch-all arm is required"
    )]
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.search_mode {
            return match key.code {
                KeyCode::Esc => {
                    self.exit_search_mode();
                    Action::ExitSearchMode
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                    Action::None
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.update_filtered_indices();
                    Action::None
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.update_filtered_indices();
                    Action::None
                }
                KeyCode::Up => {
                    self.move_up();
                    Action::None
                }
                KeyCode::Down => {
                    self.move_down();
                    Action::None
                }
                _ => Action::None,
            };
        }

        // `w` (dispatch a worker) and `r` (refresh) work from any pane.
        match key.code {
            KeyCode::Char('w') => {
                self.dispatch_worker();
                return Action::None;
            }
            KeyCode::Char('r') => {
                self.refresh();
                return Action::None;
            }
            _ => {}
        }

        match self.focus {
            Focus::List => self.handle_list_key(key),
            Focus::Detail => self.handle_detail_key(key),
            Focus::Workers => self.handle_workers_key(key),
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        // States with no selectable ticket fill the whole panel.
        if let Some(text) = self.full_area_message() {
            let paragraph = Paragraph::new(text)
                .block(self.list_block(focused))
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
            return;
        }

        // Three panes: ticket list (top-left), workers (bottom-left), and the
        // selected ticket's detail (right).
        let cols = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);
        let left = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(cols[0]);
        self.render_list(frame, left[0], focused && self.focus == Focus::List);
        self.render_workers(frame, left[1], focused && self.focus == Focus::Workers);
        self.render_detail(frame, cols[1], focused && self.focus == Focus::Detail);
    }
}

impl Notion {
    /// Handles a key while the ticket list is focused.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode is non_exhaustive, a catch-all arm is required"
    )]
    fn handle_list_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                Action::None
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                self.set_focus(Focus::Detail);
                Action::None
            }
            KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('/') => {
                self.enter_search_mode();
                Action::EnterSearchMode
            }
            KeyCode::Char('a') => {
                self.toggle_unassigned();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Handles a key while the detail pane is focused.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode is non_exhaustive, a catch-all arm is required"
    )]
    fn handle_detail_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                self.detail_scroll = self.detail_scroll.saturating_add(10);
                Action::None
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                self.set_focus(Focus::List);
                Action::None
            }
            KeyCode::Char('o') => self.open_selected(),
            _ => Action::None,
        }
    }

    /// Handles a key while the workers pane is focused.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "crossterm KeyCode is non_exhaustive, a catch-all arm is required"
    )]
    fn handle_workers_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.worker_up();
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.worker_down();
                Action::None
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected_worker_log(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                self.set_focus(Focus::List);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Builds the bordered block for the ticket list, with title and search hint.
    fn list_block(&self, focused: bool) -> Block<'static> {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let shown = self.filtered_indices.len();
        let total = self.tickets.len();
        let filter_hint = if self.show_unassigned { "all" } else { "assigned" };
        let title = format!(" \u{f0e7} Tickets ({shown}/{total}, {filter_hint}) ");
        let mut block =
            Block::default().borders(Borders::ALL).border_style(border_style).title(title);
        if self.search_mode {
            let search_title = format!(" /{} ", self.search_query);
            block = block
                .title_bottom(Line::from(search_title).style(Style::default().fg(Color::Yellow)));
        }
        block
    }

    /// Returns a full-panel message for states with no selectable ticket.
    fn full_area_message(&self) -> Option<String> {
        match &self.status {
            Status::NoToken => Some(
                "  Notion token missing.\n\n  Set NOTION_TOKEN in your environment (a read-only \
                 internal integration), share the tickets page with it, then press 'r'.\n\n  \
                 Optional: NOTION_PAGE_ID to point at another page or database."
                    .to_owned(),
            ),
            Status::Loading if self.tickets.is_empty() => {
                Some("  Loading tickets\u{2026}".to_owned())
            }
            Status::Failed(e) if self.tickets.is_empty() => {
                Some(format!("  Failed to load tickets:\n\n  {e}\n\n  Press 'r' to retry."))
            }
            Status::Ready | Status::Loading | Status::Failed(_) => {
                self.filtered_indices.is_empty().then(|| {
                    if self.show_unassigned {
                        "  No tickets.".to_owned()
                    } else {
                        "  No assigned tickets \u{2014} press 'a' to show all.".to_owned()
                    }
                })
            }
        }
    }

    /// Renders the ticket list (left pane).
    fn render_list(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let block = self.list_block(focused);

        let items: Vec<ListItem<'_>> = self
            .filtered_indices
            .iter()
            .filter_map(|&idx| self.tickets.get(idx))
            .map(|t| {
                let mut spans: Vec<Span<'_>> = Vec::with_capacity(3);
                let marker = if t.assigned() { "\u{f007}" } else { "\u{f10c}" };
                let marker_color = if t.assigned() { orange } else { Color::DarkGray };
                spans.push(Span::styled(format!(" {marker} "), Style::default().fg(marker_color)));
                if let Some(status) = &t.status {
                    spans.push(Span::styled(
                        format!("[{status}] "),
                        Style::default().fg(Color::Rgb(0x6f, 0x42, 0xc1)),
                    ));
                }
                spans.push(Span::styled(t.title.clone(), Style::default().fg(Color::Gray)));
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

    /// Renders the selected ticket's detail (right pane): properties then body.
    fn render_detail(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" \u{f02d} Ticket ");

        let Some(t) = self.selected_ticket() else {
            let paragraph = Paragraph::new("  Select a ticket.")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        };

        let mut lines: Vec<Line<'_>> = Vec::new();
        lines.push(Line::from(Span::styled(
            t.title.clone(),
            Style::default().fg(orange).add_modifier(Modifier::BOLD),
        )));

        let status = t.status.clone().unwrap_or_else(|| "\u{2014}".to_owned());
        lines.push(detail_field("Status", &status));
        let assignees = if t.assigned() { t.assignees.join(", ") } else { "\u{2014}".to_owned() };
        lines.push(detail_field("Assignees", &assignees));

        for (name, value) in &t.fields {
            lines.push(detail_field(name, value));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{2500}\u{2500} Content \u{2500}\u{2500}",
            Style::default().fg(Color::DarkGray),
        )));

        match self.details.get(&t.id) {
            Some(DetailState::Ready(body)) if !body.is_empty() => {
                for line in body {
                    lines.push(Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
            Some(DetailState::Ready(_)) => {
                lines.push(Line::from(Span::styled(
                    "(empty page)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Some(DetailState::Failed(e)) => {
                lines.push(Line::from(Span::styled(
                    format!("content error: {e}"),
                    Style::default().fg(Color::Rgb(0xdc, 0x35, 0x45)),
                )));
            }
            Some(DetailState::Loading) | None => {
                lines.push(Line::from(Span::styled(
                    "Loading\u{2026}",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0));
        frame.render_widget(paragraph, area);
    }

    /// Renders the workers pane (bottom-left): active workers then history.
    fn render_workers(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let blue = Color::Rgb(0x2a, 0x9d, 0xf4);
        let green = Color::Rgb(0x28, 0xa7, 0x45);
        let red = Color::Rgb(0xdc, 0x35, 0x45);
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);

        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let title = if self.finished.is_empty() {
            format!(" \u{f085} Workers ({}) ", self.workers.len())
        } else {
            format!(
                " \u{f085} Workers ({} active \u{00b7} {} done) ",
                self.workers.len(),
                self.finished.len()
            )
        };
        let block = Block::default().borders(Borders::ALL).border_style(border_style).title(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Reserve the last inner row for a status / hint line.
        let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
        let hint = if let Some(message) = &self.message {
            Line::from(format!(" {message} ")).style(Style::default().fg(orange))
        } else if focused {
            Line::from(vec![
                Span::styled(
                    " Enter ",
                    Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
                ),
                Span::styled("open log", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::styled(" w ", Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(
                    "run a worker on the selected ticket",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        };
        frame.render_widget(Paragraph::new(hint), rows[1]);

        if self.workers.is_empty() && self.finished.is_empty() {
            let paragraph =
                Paragraph::new(" No workers yet ").style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, rows[0]);
            return;
        }

        let mut items: Vec<ListItem<'_>> = self
            .workers
            .iter()
            .map(|worker| {
                let elapsed = worker.started.elapsed().as_secs();
                ListItem::new(Line::from(vec![
                    Span::styled(
                        " \u{25d0} ",
                        Style::default().fg(blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(truncate(&worker.title, 22), Style::default().fg(Color::White)),
                    Span::styled(
                        format!("  pid {} {elapsed}s", worker.child.id()),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        for finished in self.finished.iter().rev() {
            let (glyph, color, label) = if finished.success {
                ("\u{25cf}", green, "done")
            } else {
                ("\u{2717}", red, "failed")
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" {glyph} "), Style::default().fg(color)),
                Span::styled(truncate(&finished.title, 22), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("  {label} {}s", finished.elapsed_secs),
                    Style::default().fg(color),
                ),
            ])));
        }

        let highlight_style = if focused {
            Style::default()
                .bg(Color::Rgb(0xe6, 0x5a, 0x3d))
                .fg(Color::Rgb(0x1a, 0x12, 0x0f))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let list = List::new(items).highlight_style(highlight_style);
        frame.render_stateful_widget(list, rows[0], &mut self.worker_state);
    }
}

/// Truncates a string to `max` characters, adding an ellipsis when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    } else {
        s.to_owned()
    }
}

/// Builds a `name: value` detail line with a dimmed label.
fn detail_field<'a>(name: &str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{name}: "), Style::default().fg(Color::DarkGray)),
        Span::styled(value.to_owned(), Style::default().fg(Color::Gray)),
    ])
}

/// Builds the structured prompt handed to a `claude` worker for a ticket.
///
/// The worker is told to spec the task first and stop at the spec when it is
/// under-specified, otherwise to implement and verify it against the
/// repository's own standards.
fn build_worker_prompt(ticket: &Ticket, body: &[String]) -> String {
    let status = ticket.status.as_deref().map_or("(none)", |s| s);
    let assignees =
        if ticket.assigned() { ticket.assignees.join(", ") } else { "(none)".to_owned() };

    let mut properties = String::new();
    for (name, value) in &ticket.fields {
        properties.push_str(&format!("  - {name}: {value}\n"));
    }
    if properties.is_empty() {
        properties.push_str("  (none)\n");
    }

    let content = if body.is_empty() {
        "(no description on the Notion page)".to_owned()
    } else {
        body.join("\n")
    };

    format!(
        "You are an autonomous engineering worker running inside a software repository. A task has \
been pulled from the team's READ-ONLY Notion board: you cannot and must not write anything back to \
Notion. Your only output is changes to this repository's working tree (and, for specs, a file under \
`.cawd/specs/`).\n\n\
== TASK ==\n\
Title: {title}\n\
Status: {status}\n\
Assignees: {assignees}\n\
Properties:\n{properties}\
Description / content (from Notion):\n{content}\n\n\
== STEP 0 — TRIAGE (decide the path and state it explicitly) ==\n\
Read the task carefully, then classify it into exactly one of:\n\
  (A) ACTIONABLE — a bug fix, cleanup, refactor, or style/copy tweak. Titles tagged [BUG], [FIX], \
[CLEAN], [REFACTOR] or a `Type` of bug/cleanup are category A: DEFAULT them to IMPLEMENT and make a \
genuine best-effort fix even if the description is terse — investigate the codebase to fill the gaps \
yourself. -> IMPLEMENT.\n\
  (B) COMPLEX — a feature, an architectural or behavioural change, or anything genuinely large or \
multi-step. -> SPEC-ONLY.\n\
Only fall back to SPEC-ONLY for a category-A task if, after actually reading the code, you truly \
cannot determine a safe change. Do NOT choose SPEC-ONLY merely because the Notion text is short. \
Output your decision on the first line as `TRIAGE: IMPLEMENT` or `TRIAGE: SPEC-ONLY`, followed by one \
sentence of justification.\n\
IMPORTANT: all work happens in the repository you are running in right now. If the task clearly \
concerns a DIFFERENT codebase than the one present here (you can find nothing related to it at all), \
do not invent changes — take SPEC-ONLY and say so plainly.\n\n\
== IMPLEMENT path (category A) ==\n\
1. UNDERSTAND — locate the relevant code, confirm the exact root cause or change required, and \
reproduce the bug first when applicable.\n\
2. IMPLEMENT — make the SMALLEST change that resolves it, matching the surrounding code style and the \
repository's standards (read CLAUDE.md / AGENTS.md if present; for this Rust repo: 2024 idioms, \
`tracing` not `println!`, `#[must_use]` where relevant, doc comments on public items, conventional \
commits). You MUST actually edit files in the working tree — finishing the IMPLEMENT path without \
changing any file is a failure; if you genuinely cannot, switch to SPEC-ONLY and explain why.\n\
3. VERIFY — run the project's checks and make them pass: prefer `make lint` and `make test` when a \
Makefile exists, otherwise `cargo +nightly clippy --workspace --all-targets --all-features -- -D \
warnings` and `cargo nextest run --workspace`. Fix anything that fails before finishing.\n\
4. REPORT — summarize the root cause, the files you changed, and the lint/test results.\n\n\
== SPEC-ONLY path (for category B) ==\n\
1. Write a spec to `.cawd/specs/<slug>.md` covering: the problem, the expected behaviour, acceptance \
criteria, the files/areas likely involved, a proposed approach, risks, and an explicit list of open \
questions / missing information.\n\
2. DO NOT modify any source code.\n\
3. REPORT that this is spec-only and exactly what must be clarified before implementation can start.\n\n\
== CONSTRAINTS ==\n\
- Work only inside this repository; never modify Notion or anything outside the repo.\n\
- Do NOT commit, push, or open pull requests; leave changes in the working tree.\n\
- Prefer the smallest change that satisfies the task.\n",
        title = ticket.title,
    )
}

// ============================================================================
// Background network worker
// ============================================================================

/// Request and result channel endpoints connecting the UI to the worker thread.
type WorkerChannels = (Sender<Command>, Receiver<FetchResult>);

/// Spawns the worker thread and returns the request/result channel endpoints.
///
/// The thread owns a single-threaded Tokio runtime and a `reqwest` client, and
/// services one [`Command`] per request until the request channel is dropped.
fn spawn_worker(token: String, page_id: String) -> WorkerChannels {
    let (req_tx, req_rx) = mpsc::channel::<Command>();
    let (res_tx, res_rx) = mpsc::channel::<FetchResult>();

    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                res_tx.send(FetchResult::List(Err(format!("tokio runtime: {e}")))).ok();
                return;
            }
        };

        let client = match reqwest::Client::builder().build() {
            Ok(c) => c,
            Err(e) => {
                res_tx.send(FetchResult::List(Err(format!("http client: {e}")))).ok();
                return;
            }
        };

        while let Ok(command) = req_rx.recv() {
            let msg = match command {
                Command::Refresh => {
                    FetchResult::List(runtime.block_on(fetch_tickets(&client, &token, &page_id)))
                }
                Command::Detail(id) => {
                    let result = runtime.block_on(fetch_body(&client, &token, &id));
                    FetchResult::Detail { id, result }
                }
            };
            if res_tx.send(msg).is_err() {
                break;
            }
        }
    });

    (req_tx, res_rx)
}

/// Fetches all tickets reachable from the configured page.
///
/// Reads the page's child blocks; any inline database is queried for its rows
/// (with assignment/status properties), and child pages become bare tickets.
async fn fetch_tickets(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
) -> Result<Vec<Ticket>, String> {
    let assignee_prop = env_string("NOTION_ASSIGNEE_PROP");
    let children = fetch_block_children(client, token, page_id).await?;

    let mut tickets = Vec::new();
    let mut found_database = false;

    for child in &children {
        let block_type = child.get("type").and_then(serde_json::Value::as_str).unwrap_or_default();
        match block_type {
            "child_database" => {
                found_database = true;
                if let Some(db_id) = child.get("id").and_then(serde_json::Value::as_str) {
                    let rows = query_database(client, token, db_id).await?;
                    for row in &rows {
                        if let Some(ticket) = parse_row(row, assignee_prop.as_deref()) {
                            tickets.push(ticket);
                        }
                    }
                }
            }
            "child_page" => {
                let title = child
                    .get("child_page")
                    .and_then(|p| p.get("title"))
                    .and_then(serde_json::Value::as_str)
                    .map_or("(untitled)", |s| s)
                    .to_owned();
                let id = child
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let url = page_url(&id);
                tickets.push(Ticket {
                    id,
                    title,
                    assignees: Vec::new(),
                    status: None,
                    url,
                    fields: Vec::new(),
                });
            }
            _ => {}
        }
    }

    // Fallback: the configured id may itself point at a database rather than a
    // page wrapping one. Try querying it directly if nothing turned up.
    if !found_database &&
        tickets.is_empty() &&
        let Ok(rows) = query_database(client, token, page_id).await
    {
        for row in &rows {
            if let Some(ticket) = parse_row(row, assignee_prop.as_deref()) {
                tickets.push(ticket);
            }
        }
    }

    Ok(tickets)
}

/// Fetches every child block of a page/block, following pagination.
async fn fetch_block_children(
    client: &reqwest::Client,
    token: &str,
    block_id: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let mut results = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!("{API_BASE}/blocks/{block_id}/children?page_size=100");
        if let Some(c) = &cursor {
            url.push_str("&start_cursor=");
            url.push_str(c);
        }

        let json = get_json(client, token, &url).await?;
        collect_results(&json, &mut results);

        match next_cursor(&json) {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    Ok(results)
}

/// Queries a database's rows, following pagination.
async fn query_database(
    client: &reqwest::Client,
    token: &str,
    database_id: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let url = format!("{API_BASE}/databases/{database_id}/query");
    let mut results = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut body = serde_json::Map::new();
        body.insert("page_size".to_owned(), serde_json::Value::from(100));
        if let Some(c) = &cursor {
            body.insert("start_cursor".to_owned(), serde_json::Value::from(c.clone()));
        }

        let json = post_json(client, token, &url, &serde_json::Value::Object(body)).await?;
        collect_results(&json, &mut results);

        match next_cursor(&json) {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    Ok(results)
}

/// Appends the `results` array of a Notion list response into `out`.
fn collect_results(json: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    if let Some(arr) = json.get("results").and_then(serde_json::Value::as_array) {
        out.extend(arr.iter().cloned());
    }
}

/// Returns the pagination cursor when the response has more pages.
fn next_cursor(json: &serde_json::Value) -> Option<String> {
    if json.get("has_more").and_then(serde_json::Value::as_bool).unwrap_or_default() {
        json.get("next_cursor").and_then(serde_json::Value::as_str).map(str::to_owned)
    } else {
        None
    }
}

/// A people-typed property: its name and the names of the assigned people.
type PeopleProp<'a> = (&'a str, Vec<String>);

/// Parses a Notion database row (a page object) into a [`Ticket`].
///
/// `assignee_prop` optionally names the people property to treat as the
/// assignment field; when unset, a heuristic prefers a lead/owner-style
/// property and otherwise falls back to the first people property.
fn parse_row(page: &serde_json::Value, assignee_prop: Option<&str>) -> Option<Ticket> {
    let props = page.get("properties").and_then(serde_json::Value::as_object)?;

    let mut title = String::new();
    let mut status_label: Option<String> = None;
    let mut select_label: Option<String> = None;
    // (property name, assigned people) for every people-typed property.
    let mut people_props: Vec<PeopleProp<'_>> = Vec::new();
    // Remaining properties rendered for the detail pane.
    let mut fields: Vec<(String, String)> = Vec::new();

    for (name, value) in props {
        let prop_type = value.get("type").and_then(serde_json::Value::as_str).unwrap_or_default();
        match prop_type {
            "title" => {
                if let Some(arr) = value.get("title").and_then(serde_json::Value::as_array) {
                    title = arr
                        .iter()
                        .filter_map(|rt| rt.get("plain_text").and_then(serde_json::Value::as_str))
                        .collect();
                }
            }
            "people" => {
                let names = value
                    .get("people")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.get("name").and_then(serde_json::Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                people_props.push((name.as_str(), names));
            }
            "status" if status_label.is_none() => {
                status_label = value
                    .get("status")
                    .and_then(|s| s.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
            }
            "select" if select_label.is_none() => {
                select_label = value
                    .get("select")
                    .and_then(|s| s.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
            }
            _ => {}
        }

        // The title and status appear in the header; everything else with a
        // value is shown in the detail pane.
        if prop_type != "title" &&
            prop_type != "status" &&
            let Some(rendered) = render_property(value)
        {
            fields.push((name.clone(), rendered));
        }
    }

    // A real `status` property always wins over an arbitrary `select`.
    let status = status_label.or(select_label);
    let assignees = pick_assignees(&people_props, assignee_prop);

    if title.is_empty() {
        title = "(untitled)".to_owned();
    }

    let id = page.get("id").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let url = page.get("url").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();

    Some(Ticket { id, title, assignees, status, url, fields })
}

/// Renders a Notion property value to a short display string, when non-empty.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "Notion exposes many property types; only a subset is rendered"
)]
fn render_property(value: &serde_json::Value) -> Option<String> {
    let prop_type = value.get("type").and_then(serde_json::Value::as_str)?;
    let rendered = match prop_type {
        "select" | "status" => value
            .get(prop_type)
            .and_then(|s| s.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        "multi_select" | "people" => {
            let names = join_named(value.get(prop_type));
            (!names.is_empty()).then_some(names)
        }
        "rich_text" | "title" => {
            let text = plain_text(value.get(prop_type));
            (!text.is_empty()).then_some(text)
        }
        "date" => {
            let date = value.get("date");
            let start = date.and_then(|d| d.get("start")).and_then(serde_json::Value::as_str);
            let end = date.and_then(|d| d.get("end")).and_then(serde_json::Value::as_str);
            match (start, end) {
                (Some(s), Some(e)) => Some(format!("{s} \u{2192} {e}")),
                (Some(s), None) => Some(s.to_owned()),
                _ => None,
            }
        }
        "number" => value.get("number").and_then(serde_json::Value::as_f64).map(format_number),
        "checkbox" => value
            .get("checkbox")
            .and_then(serde_json::Value::as_bool)
            .map(|b| if b { "\u{2713}".to_owned() } else { "\u{2717}".to_owned() }),
        "url" | "email" | "phone_number" | "created_time" | "last_edited_time" => {
            value.get(prop_type).and_then(serde_json::Value::as_str).map(str::to_owned)
        }
        "relation" => match value.get("relation").and_then(serde_json::Value::as_array) {
            Some(arr) if !arr.is_empty() => Some(format!("{} linked", arr.len())),
            _ => None,
        },
        "formula" => render_formula(value.get("formula")),
        _ => None,
    };
    rendered.filter(|s| !s.is_empty())
}

/// Joins the `name` fields of an array of objects (people, multi-select…).
fn join_named(array: Option<&serde_json::Value>) -> String {
    array
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Concatenates the `plain_text` of a Notion rich-text array.
fn plain_text(array: Option<&serde_json::Value>) -> String {
    array
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|rt| rt.get("plain_text").and_then(serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

/// Renders a number without a trailing `.0` for whole values.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Renders a Notion `formula` property value.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "Notion formula results have several variants; only common ones are rendered"
)]
fn render_formula(value: Option<&serde_json::Value>) -> Option<String> {
    let formula = value?;
    let kind = formula.get("type").and_then(serde_json::Value::as_str)?;
    match kind {
        "string" => formula.get("string").and_then(serde_json::Value::as_str).map(str::to_owned),
        "number" => formula.get("number").and_then(serde_json::Value::as_f64).map(format_number),
        "boolean" => formula
            .get("boolean")
            .and_then(serde_json::Value::as_bool)
            .map(|b| if b { "\u{2713}".to_owned() } else { "\u{2717}".to_owned() }),
        "date" => formula
            .get("date")
            .and_then(|d| d.get("start"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// Chooses which people property holds a ticket's assignees.
///
/// Preference order: an explicit `assignee_prop` name, then a property whose
/// name reads like an assignment field (lead/owner/assignee/…), then the first
/// people property present.
fn pick_assignees(people: &[PeopleProp<'_>], assignee_prop: Option<&str>) -> Vec<String> {
    if let Some(wanted) = assignee_prop &&
        let Some((_, names)) = people.iter().find(|(n, _)| n.eq_ignore_ascii_case(wanted))
    {
        return names.clone();
    }

    people
        .iter()
        .find(|(n, _)| name_looks_like_assignee(n))
        .or_else(|| people.first())
        .map(|(_, names)| names.clone())
        .unwrap_or_default()
}

/// Whether a property name reads like an assignment field.
fn name_looks_like_assignee(name: &str) -> bool {
    const HINTS: [&str; 5] = ["assign", "lead", "owner", "responsible", "attribu"];
    let lower = name.to_lowercase();
    HINTS.iter().any(|hint| lower.contains(hint))
}

/// Builds a Notion page URL from a (possibly dashed) page id.
fn page_url(id: &str) -> String {
    let dashless: String = id.chars().filter(|c| *c != '-').collect();
    format!("https://www.notion.so/{dashless}")
}

/// Fetches a ticket page's body and renders its top-level blocks to text lines.
async fn fetch_body(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
) -> Result<Vec<String>, String> {
    let blocks = fetch_block_children(client, token, page_id).await?;
    Ok(blocks.iter().filter_map(render_block).collect())
}

/// Renders a single Notion block to a plain-text line, when it has content.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "Notion has many block types; unsupported ones fall back to their text"
)]
fn render_block(block: &serde_json::Value) -> Option<String> {
    let block_type = block.get("type").and_then(serde_json::Value::as_str)?;
    if block_type == "divider" {
        return Some("\u{2500}\u{2500}\u{2500}".to_owned());
    }

    let body = block.get(block_type)?;
    let text = plain_text(body.get("rich_text"));

    let line = match block_type {
        "heading_1" => format!("# {text}"),
        "heading_2" => format!("## {text}"),
        "heading_3" => format!("### {text}"),
        "bulleted_list_item" | "numbered_list_item" => format!("\u{2022} {text}"),
        "to_do" => {
            let checked = body.get("checked").and_then(serde_json::Value::as_bool) == Some(true);
            let mark = if checked { "\u{2611}" } else { "\u{2610}" };
            format!("{mark} {text}")
        }
        "quote" => format!("\u{258c} {text}"),
        "callout" => format!("\u{1f4a1} {text}"),
        "toggle" => format!("\u{25b8} {text}"),
        "code" => format!("\u{2502} {text}"),
        _ => text,
    };

    (!line.trim().is_empty()).then_some(line)
}

/// Performs an authenticated `GET` and parses the JSON body.
async fn get_json(
    client: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<serde_json::Value, String> {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", NOTION_VERSION)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_response(resp).await
}

/// Performs an authenticated `POST` with a JSON body and parses the response.
async fn post_json(
    client: &reqwest::Client,
    token: &str,
    url: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", NOTION_VERSION)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_response(resp).await
}

/// Validates the HTTP status and deserializes the JSON body.
async fn parse_response(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let snippet: String = text.chars().take(200).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assigned_row_with_status() {
        let page = serde_json::json!({
            "url": "https://www.notion.so/abc",
            "properties": {
                "Name": { "type": "title", "title": [{ "plain_text": "Fix login" }] },
                "Assignee": { "type": "people", "people": [{ "name": "Alice" }, { "name": "Bob" }] },
                "Stage": { "type": "status", "status": { "name": "In progress" } }
            }
        });

        let ticket = parse_row(&page, None).expect("row parses");
        assert_eq!(ticket.title, "Fix login");
        assert_eq!(ticket.assignees, vec!["Alice".to_owned(), "Bob".to_owned()]);
        assert_eq!(ticket.status.as_deref(), Some("In progress"));
        assert!(ticket.assigned());
        assert_eq!(ticket.url, "https://www.notion.so/abc");
    }

    #[test]
    fn prefers_status_over_select_and_lead_over_from() {
        // Mirrors the real "TASK RUN TECH" schema: a `select` (Difficulty)
        // sorts before `Status` alphabetically, and `From` before `Tech lead`.
        let page = serde_json::json!({
            "properties": {
                "Task name": { "type": "title", "title": [{ "plain_text": "Swap bug" }] },
                "Difficulty": { "type": "select", "select": { "name": "Hard" } },
                "Status": { "type": "status", "status": { "name": "To dev" } },
                "From": { "type": "people", "people": [{ "name": "Requester" }] },
                "Tech lead": { "type": "people", "people": [{ "name": "Mateo" }] }
            }
        });

        let ticket = parse_row(&page, None).expect("row parses");
        assert_eq!(ticket.status.as_deref(), Some("To dev"));
        assert_eq!(ticket.assignees, vec!["Mateo".to_owned()]);
        // Non-title, non-status properties land in the detail fields.
        assert!(ticket.fields.contains(&("Difficulty".to_owned(), "Hard".to_owned())));
    }

    #[test]
    fn renders_common_block_types() {
        let h1 = serde_json::json!({
            "type": "heading_1",
            "heading_1": { "rich_text": [{ "plain_text": "Title" }] }
        });
        assert_eq!(render_block(&h1).as_deref(), Some("# Title"));

        let todo = serde_json::json!({
            "type": "to_do",
            "to_do": { "rich_text": [{ "plain_text": "task" }], "checked": true }
        });
        assert_eq!(render_block(&todo).as_deref(), Some("\u{2611} task"));

        let empty = serde_json::json!({
            "type": "paragraph",
            "paragraph": { "rich_text": [] }
        });
        assert!(render_block(&empty).is_none());
    }

    #[test]
    fn assignee_override_selects_named_property() {
        let page = serde_json::json!({
            "properties": {
                "Task name": { "type": "title", "title": [{ "plain_text": "X" }] },
                "From": { "type": "people", "people": [{ "name": "Requester" }] },
                "Tech lead": { "type": "people", "people": [{ "name": "Mateo" }] }
            }
        });

        let ticket = parse_row(&page, Some("From")).expect("row parses");
        assert_eq!(ticket.assignees, vec!["Requester".to_owned()]);
    }

    #[test]
    fn unassigned_row_falls_back_to_untitled() {
        let page = serde_json::json!({
            "properties": { "Name": { "type": "title", "title": [] } }
        });

        let ticket = parse_row(&page, None).expect("row parses");
        assert_eq!(ticket.title, "(untitled)");
        assert!(!ticket.assigned());
        assert!(ticket.status.is_none());
    }

    #[test]
    fn row_without_properties_is_skipped() {
        let page = serde_json::json!({ "id": "x" });
        assert!(parse_row(&page, None).is_none());
    }

    #[test]
    fn normalizes_url_to_dashless_id() {
        let id = normalize_page_id("Task-Manager-3353b9213ea780289800fee6656bea2b");
        assert_eq!(id, "3353b9213ea780289800fee6656bea2b");
    }

    #[test]
    fn sorts_assigned_before_unassigned() {
        let mut tickets = vec![
            Ticket {
                id: String::new(),
                title: "b".to_owned(),
                assignees: vec![],
                status: None,
                url: String::new(),
                fields: vec![],
            },
            Ticket {
                id: String::new(),
                title: "a".to_owned(),
                assignees: vec!["x".to_owned()],
                status: None,
                url: String::new(),
                fields: vec![],
            },
        ];

        sort_tickets(&mut tickets);
        assert_eq!(tickets[0].title, "a");
        assert!(tickets[0].assigned());
    }
}
