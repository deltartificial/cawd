//! Main application state and event loop.

use crate::action::Action;
use crate::components::code_viewer::CodeViewer;
use crate::components::file_tree::FileTree;
use crate::components::git_status::GitStatus;
use crate::components::help_bar::HelpBar;
use crate::components::search_modal::SearchModal;
use crate::components::Component;
use crate::tui::Tui;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::PathBuf;
use std::time::Instant;

/// The currently active panel in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// File tree explorer panel.
    FileTree,
    /// Git status panel showing changed files.
    GitStatus,
    /// Code viewer panel with syntax highlighting.
    CodeViewer,
}

/// Main application state container.
///
/// Manages all UI components, handles event routing, and maintains
/// the overall application state including which panel is focused.
pub struct App {
    file_tree: FileTree,
    git_status: GitStatus,
    code_viewer: CodeViewer,
    help_bar: HelpBar,
    search_modal: SearchModal,
    active_panel: Panel,
    should_quit: bool,
    #[allow(dead_code)]
    root: PathBuf,
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
    pub fn new(path: PathBuf) -> color_eyre::Result<Self> {
        let root = if path.is_file() {
            path.parent().unwrap_or(&path).to_path_buf()
        } else {
            path.clone()
        };

        Ok(Self {
            file_tree: FileTree::new(path.clone())?,
            git_status: GitStatus::new(root.clone()),
            code_viewer: CodeViewer::new(),
            help_bar: HelpBar::new(),
            search_modal: SearchModal::new(root.clone()),
            active_panel: Panel::FileTree,
            should_quit: false,
            root,
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
    pub fn run(&mut self, terminal: &mut Tui) -> color_eyre::Result<()> {
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
    fn render(&mut self, frame: &mut ratatui::Frame) {
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let tabs_area = main_layout[0];
        let content_area = main_layout[1];
        let help_area = main_layout[2];

        self.render_tabs(frame, tabs_area);

        let content_layout = Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .split(content_area);

        let left_layout = Layout::vertical([
            Constraint::Percentage(75),
            Constraint::Percentage(25),
        ])
        .split(content_layout[0]);

        self.file_tree
            .render(frame, left_layout[0], self.active_panel == Panel::FileTree);
        self.git_status
            .render(frame, left_layout[1], self.active_panel == Panel::GitStatus);
        self.code_viewer
            .render(frame, content_layout[1], self.active_panel == Panel::CodeViewer);

        let search_mode = self.file_tree.is_search_mode()
            || self.git_status.is_search_mode()
            || self.code_viewer.is_search_mode();
        let in_code_viewer = self.active_panel == Panel::CodeViewer && self.code_viewer.has_file();
        let in_git_status = self.active_panel == Panel::GitStatus;
        self.help_bar.set_context(search_mode, in_code_viewer, in_git_status);
        self.help_bar.render(frame, help_area);

        self.search_modal.render(frame);
    }

    /// Renders the tab bar showing panel names.
    ///
    /// Active panel is highlighted with orange background.
    fn render_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
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

        let code_style = if self.active_panel == Panel::CodeViewer {
            Style::default().fg(dark_text).bg(orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let file_name = self.code_viewer.file_name().unwrap_or("Code");

        let tabs_line = Line::from(vec![
            Span::styled(" \u{f07b} Explorer ", explorer_style),
            Span::raw(" "),
            Span::styled(" \u{f126} Changes ", git_style),
            Span::raw(" "),
            Span::styled(format!(" \u{f15b} {} ", file_name), code_style),
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
        let timeout = if !self.code_viewer.has_file() {
            std::time::Duration::from_millis(50)
        } else {
            std::time::Duration::from_millis(100)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }

                if self.search_modal.active {
                    if let Some(path) = self.search_modal.handle_key(key) {
                        if path.is_file() {
                            if let Err(e) = self.code_viewer.load_file(path) {
                                self.code_viewer.show_error(&e.to_string());
                            }
                            self.active_panel = Panel::CodeViewer;
                        }
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

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                    self.search_modal.open();
                    return Ok(());
                }

                if key.code == KeyCode::Char('q')
                    || (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c'))
                {
                    self.should_quit = true;
                    return Ok(());
                }

                if key.code == KeyCode::Tab {
                    self.active_panel = match self.active_panel {
                        Panel::FileTree => Panel::GitStatus,
                        Panel::GitStatus => Panel::CodeViewer,
                        Panel::CodeViewer => Panel::FileTree,
                    };
                    return Ok(());
                }

                if key.code == KeyCode::BackTab {
                    self.active_panel = match self.active_panel {
                        Panel::FileTree => Panel::CodeViewer,
                        Panel::GitStatus => Panel::FileTree,
                        Panel::CodeViewer => Panel::GitStatus,
                    };
                    return Ok(());
                }

                let action = match self.active_panel {
                    Panel::FileTree => self.file_tree.handle_key_event(key),
                    Panel::GitStatus => self.git_status.handle_key_event(key),
                    Panel::CodeViewer => self.code_viewer.handle_key_event(key),
                };

                self.handle_action(action)?;
            }
        }

        if self.last_git_refresh.elapsed().as_secs() >= 2 {
            self.git_status.refresh();
            self.last_git_refresh = Instant::now();
        }

        Ok(())
    }

    /// Processes an action returned by a component.
    ///
    /// # Parameters
    ///
    /// * `action` - The action to process.
    fn handle_action(&mut self, action: Action) -> color_eyre::Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
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
            _ => {}
        }
        Ok(())
    }
}
