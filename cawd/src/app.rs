//! Main application state and event loop.

use crate::{
    action::Action,
    components::{
        Component,
        code_viewer::CodeViewer,
        file_tree::FileTree,
        git_status::GitStatus,
        help_bar::{HelpBar, HelpContext},
        notion::Notion,
        review::Review,
        search_modal::SearchModal,
    },
    tui::Tui,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{path::PathBuf, time::Instant};

/// The currently active panel in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Panel {
    /// File tree explorer panel.
    FileTree,
    /// Git status panel showing changed files.
    GitStatus,
    /// Review panel listing code annotations.
    Review,
    /// Code viewer panel with syntax highlighting.
    CodeViewer,
    /// Notion panel listing tickets from a configured page.
    Notion,
}

/// Main application state container.
///
/// Manages all UI components, handles event routing, and maintains
/// the overall application state including which panel is focused.
pub(crate) struct App {
    file_tree: FileTree,
    git_status: GitStatus,
    review: Review,
    code_viewer: CodeViewer,
    notion: Notion,
    help_bar: HelpBar,
    search_modal: SearchModal,
    active_panel: Panel,
    should_quit: bool,
    last_git_refresh: Instant,
}

impl App {
    /// Creates a new application instance.
    ///
    /// # Parameters
    ///
    /// * `path` - The root directory or file path to open.
    ///
    /// # Returns
    ///
    /// Returns a configured `App` instance, or an error if initialization fails.
    pub(crate) fn new(path: PathBuf) -> color_eyre::Result<Self> {
        let root = if path.is_file() {
            path.parent().map_or(path.as_path(), |it| it).to_path_buf()
        } else {
            path.clone()
        };

        Ok(Self {
            file_tree: FileTree::new(path)?,
            git_status: GitStatus::new(root.clone()),
            review: Review::new(root.clone()),
            code_viewer: CodeViewer::new(root.clone()),
            notion: Notion::new(root.clone()),
            help_bar: HelpBar::new(),
            search_modal: SearchModal::new(root),
            active_panel: Panel::FileTree,
            should_quit: false,
            last_git_refresh: Instant::now(),
        })
    }

    /// Runs the main application loop.
    ///
    /// Continuously renders the UI and handles events until the user quits.
    ///
    /// # Parameters
    ///
    /// * `terminal` - Mutable reference to the terminal instance.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on normal exit, or an error if rendering/events fail.
    pub(crate) fn run(&mut self, terminal: &mut Tui) -> color_eyre::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    /// Renders all UI components to the terminal frame.
    ///
    /// Layout structure:
    /// - Top: Tab bar (1 line)
    /// - Middle: Content area split into left (30%) and right (70%)
    ///   - Left: File tree (75%) and Git status (25%)
    ///   - Right: Code viewer
    /// - Bottom: Help bar (1 line)
    fn render(&mut self, frame: &mut ratatui::Frame<'_>) {
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        let tabs_area = main_layout[0];
        let content_area = main_layout[1];
        let help_area = main_layout[2];

        self.render_tabs(frame, tabs_area);

        // The Notion panel is unrelated to the code view, so it takes the whole
        // content area instead of sharing the left/right split.
        if self.active_panel == Panel::Notion {
            self.notion.render(frame, content_area, true);
            self.help_bar.set_context(HelpContext {
                search_mode: self.notion.is_search_mode(),
                in_notion: true,
                ..HelpContext::default()
            });
            self.help_bar.render(frame, help_area);
            self.search_modal.render(frame);
            return;
        }

        let content_layout =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(content_area);

        if self.active_panel == Panel::Review {
            self.review.render(frame, content_layout[0], true);
        } else if self.active_panel == Panel::GitStatus {
            // Changes takes the whole left column: changed files on top, recent
            // commits in a card below.
            let left_layout =
                Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(content_layout[0]);

            self.git_status.render(frame, left_layout[0], true);
            self.git_status.render_commits(frame, left_layout[1]);
        } else {
            let left_layout =
                Layout::vertical([Constraint::Percentage(75), Constraint::Percentage(25)])
                    .split(content_layout[0]);

            self.file_tree.render(frame, left_layout[0], self.active_panel == Panel::FileTree);
            self.git_status.render(frame, left_layout[1], false);
        }

        self.code_viewer.render(frame, content_layout[1], self.active_panel == Panel::CodeViewer);

        let search_mode = self.file_tree.is_search_mode() ||
            self.git_status.is_search_mode() ||
            self.code_viewer.is_search_mode();
        self.help_bar.set_context(HelpContext {
            search_mode,
            in_code_viewer: self.active_panel == Panel::CodeViewer && self.code_viewer.has_file(),
            in_git_status: self.active_panel == Panel::GitStatus,
            in_review: self.active_panel == Panel::Review,
            in_notion: false,
        });
        self.help_bar.render(frame, help_area);

        self.search_modal.render(frame);
    }

    /// Renders the tab bar showing panel names.
    ///
    /// Active panel is highlighted with orange background.
    fn render_tabs(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let dark_text = Color::Rgb(0x1a, 0x12, 0x0f);

        let explorer_style = if self.active_panel == Panel::FileTree {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let git_style = if self.active_panel == Panel::GitStatus {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let review_style = if self.active_panel == Panel::Review {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let code_style = if self.active_panel == Panel::CodeViewer {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let notion_style = if self.active_panel == Panel::Notion {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let file_name = self.code_viewer.file_name().map_or("Code", |it| it);

        let tabs_line = Line::from(vec![
            Span::styled(" 1 \u{f07b} Explorer ", explorer_style),
            Span::raw(" "),
            Span::styled(" 2 \u{f126} Changes ", git_style),
            Span::raw(" "),
            Span::styled(" 3 \u{f075} Review ", review_style),
            Span::raw(" "),
            Span::styled(format!(" 4 \u{f15b} {file_name} "), code_style),
            Span::raw(" "),
            Span::styled(" 5 \u{f0e7} Notion ", notion_style),
            Span::raw("  "),
            Span::styled("Ctrl+P: Search", Style::default().fg(Color::DarkGray)),
        ]);

        let tabs = ratatui::widgets::Paragraph::new(tabs_line)
            .style(Style::default().bg(Color::Rgb(0x1a, 0x1a, 0x2e)));

        frame.render_widget(tabs, area);
    }

    /// Handles all input events and updates application state.
    ///
    /// Event priority:
    /// 1. Search modal (when active)
    /// 2. Component search modes
    /// 3. Global shortcuts (Ctrl+P, Tab, q)
    /// 4. Active panel key handling
    fn handle_events(&mut self) -> color_eyre::Result<()> {
        let timeout = if self.code_viewer.has_file() {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_millis(50)
        };

        if event::poll(timeout)? {
            let evt = event::read()?;

            if let Event::Mouse(mouse) = evt {
                if self.code_viewer.handle_mouse_event(mouse) {
                    self.active_panel = Panel::CodeViewer;
                }
                return Ok(());
            }

            if let Event::Key(key) = evt {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }

                if self.search_modal.active {
                    if let Some(path) = self.search_modal.handle_key(key) &&
                        path.is_file()
                    {
                        if let Err(e) = self.code_viewer.load_file(path) {
                            self.code_viewer.show_error(&e.to_string());
                        }
                        self.active_panel = Panel::CodeViewer;
                    }
                    return Ok(());
                }

                if self.file_tree.is_search_mode() {
                    let action = self.file_tree.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                if self.git_status.is_search_mode() {
                    let action = self.git_status.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                if self.code_viewer.is_search_mode() {
                    let action = self.code_viewer.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                if self.code_viewer.is_commenting() {
                    let action = self.code_viewer.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                    self.search_modal.open();
                    return Ok(());
                }

                if key.code == KeyCode::Char('q') ||
                    (key.modifiers.contains(KeyModifiers::CONTROL) &&
                        key.code == KeyCode::Char('c'))
                {
                    self.should_quit = true;
                    return Ok(());
                }

                // Tab/BackTab only move focus *within* the active panel;
                // switching panels is done with the number keys (1-5). Only
                // Notion has sub-panes, so Tab cycles those and is a no-op
                // in the other panels.
                if key.code == KeyCode::Tab {
                    if self.active_panel == Panel::Notion {
                        self.notion.focus_next();
                    }
                    return Ok(());
                }

                if key.code == KeyCode::BackTab {
                    if self.active_panel == Panel::Notion {
                        self.notion.focus_prev();
                    }
                    return Ok(());
                }

                // Direct panel jumps: 1 Explorer, 2 Changes, 3 Review, 4 File,
                // 5 Notion.
                if let KeyCode::Char(digit @ '1'..='5') = key.code {
                    self.active_panel = match digit {
                        '1' => Panel::FileTree,
                        '2' => Panel::GitStatus,
                        '3' => Panel::Review,
                        '4' => Panel::CodeViewer,
                        _ => Panel::Notion,
                    };
                    self.on_panel_activated();
                    return Ok(());
                }

                let action = match self.active_panel {
                    Panel::FileTree => self.file_tree.handle_key_event(key),
                    Panel::GitStatus => self.git_status.handle_key_event(key),
                    Panel::Review => self.review.handle_key_event(key),
                    Panel::CodeViewer => self.code_viewer.handle_key_event(key),
                    Panel::Notion => self.notion.handle_key_event(key),
                };

                self.handle_action(action)?;

                // An input event was handled this tick. Skip the periodic
                // maintenance below so navigation stays responsive — git
                // refresh spawns a subprocess and must never run inline with
                // a keypress.
                return Ok(());
            }
        }

        // Idle tick (no key/mouse event waiting): drain Notion fetch results
        // (cheap, runs every tick so the panel stays responsive).
        self.notion.poll();

        // Periodic maintenance.
        if self.last_git_refresh.elapsed().as_secs() >= 2 {
            self.git_status.refresh();
            self.review.poll_workers();
            self.review.refresh();
            // Keep the in-file annotation overlays in sync with status changes
            // made from the Review panel or by workers.
            self.code_viewer.refresh_annotations();
            self.last_git_refresh = Instant::now();
        }

        Ok(())
    }

    /// Refreshes side state when a panel gains focus via Tab/number keys.
    fn on_panel_activated(&mut self) {
        match self.active_panel {
            Panel::Review => self.review.refresh(),
            Panel::Notion => {
                self.notion.refresh();
                self.notion.focus_list();
            }
            Panel::FileTree | Panel::GitStatus | Panel::CodeViewer => {}
        }
    }

    /// Processes an action returned by a component.
    ///
    /// # Parameters
    ///
    /// * `action` - The action to process.
    fn handle_action(&mut self, action: Action) -> color_eyre::Result<()> {
        match action {
            Action::FileSelected(path) => {
                if let Err(e) = self.code_viewer.load_file(path) {
                    self.code_viewer.show_error(&e.to_string());
                }
                self.active_panel = Panel::CodeViewer;
                self.git_status.refresh();
            }
            Action::DiffSelected(path) => {
                if let Err(e) = self.code_viewer.load_diff(path) {
                    self.code_viewer.show_error(&e.to_string());
                }
                self.active_panel = Panel::CodeViewer;
            }
            Action::CommitSelected(hash) => {
                // Open the commit's full diff in the code viewer, like a file.
                if let Err(e) = self.code_viewer.load_commit_diff(&hash) {
                    self.code_viewer.show_error(&e.to_string());
                }
                self.active_panel = Panel::CodeViewer;
            }
            Action::AnnotationOpen { path, line } => {
                // Keep focus on the review panel; show the code on the right.
                if let Err(e) = self.code_viewer.load_file(path) {
                    self.code_viewer.show_error(&e.to_string());
                } else {
                    self.code_viewer.scroll_to_line(line);
                }
            }
            Action::DispatchWorker { id, commit } => {
                // Load the just-saved annotation into the review panel, launch a
                // worker on it, and reflect the in-progress state in the viewer.
                self.review.refresh();
                self.review.dispatch_worker_for_id(&id, commit);
                self.code_viewer.refresh_annotations();
            }
            Action::OpenUrl(url) => {
                Self::open_url(&url);
            }
            Action::ToggleHidden |
            Action::EnterSearchMode |
            Action::ExitSearchMode |
            Action::None => {}
        }
        Ok(())
    }

    /// Opens a URL in the user's default browser, ignoring failures.
    ///
    /// Errors are deliberately swallowed: the TUI owns the terminal, so there
    /// is nowhere useful to surface a spawn failure, and a missing opener must
    /// not crash the app.
    fn open_url(url: &str) {
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");
        #[cfg(target_os = "linux")]
        let mut command = std::process::Command::new("xdg-open");
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        };

        command.arg(url);
        match command.spawn() {
            Ok(_) | Err(_) => {}
        }
    }
}
