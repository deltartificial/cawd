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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    FileTree,
    GitStatus,
    CodeViewer,
}

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
}

impl App {
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
        })
    }

    pub fn run(&mut self, terminal: &mut Tui) -> color_eyre::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Tabs
                Constraint::Min(1),    // Content
                Constraint::Length(1), // Help bar
            ])
            .split(frame.area());

        let tabs_area = main_layout[0];
        let content_area = main_layout[1];
        let help_area = main_layout[2];

        // Render tabs
        self.render_tabs(frame, tabs_area);

        // Content layout: left panel (30%) and code viewer (70%)
        let content_layout = Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .split(content_area);

        // Split left panel: file tree (75%) and git status (25%)
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

        // Update help bar context
        let search_mode = self.file_tree.is_search_mode()
            || self.git_status.is_search_mode()
            || self.code_viewer.is_search_mode();
        let in_code_viewer = self.active_panel == Panel::CodeViewer && self.code_viewer.has_file();
        let in_git_status = self.active_panel == Panel::GitStatus;
        self.help_bar.set_context(search_mode, in_code_viewer, in_git_status);
        self.help_bar.render(frame, help_area);

        // Render search modal on top if active
        self.search_modal.render(frame);
    }

    fn render_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
        let orange = Color::Rgb(0xff, 0x7a, 0x5c);
        let _dark_orange = Color::Rgb(0xe6, 0x5a, 0x3d);
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

    fn handle_events(&mut self) -> color_eyre::Result<()> {
        // Short poll for smooth animation when on welcome screen
        let timeout = if !self.code_viewer.has_file() {
            std::time::Duration::from_millis(50) // 20 fps for animation
        } else {
            std::time::Duration::from_millis(100)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }

                // Search modal takes priority
                if self.search_modal.active {
                    if let Some(path) = self.search_modal.handle_key(key) {
                        if path.is_file() {
                            if let Err(e) = self.code_viewer.load_file(path) {
                                eprintln!("Could not load file: {}", e);
                            }
                            self.active_panel = Panel::CodeViewer;
                        }
                    }
                    return Ok(());
                }

                // In file tree search mode
                if self.file_tree.is_search_mode() {
                    let action = self.file_tree.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                // In git status search mode
                if self.git_status.is_search_mode() {
                    let action = self.git_status.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                // In code viewer search mode
                if self.code_viewer.is_search_mode() {
                    let action = self.code_viewer.handle_key_event(key);
                    self.handle_action(action)?;
                    return Ok(());
                }

                // Global shortcuts
                // Ctrl+P to open search modal
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

                // Shift+Tab to go backwards
                if key.code == KeyCode::BackTab {
                    self.active_panel = match self.active_panel {
                        Panel::FileTree => Panel::CodeViewer,
                        Panel::GitStatus => Panel::FileTree,
                        Panel::CodeViewer => Panel::GitStatus,
                    };
                    return Ok(());
                }

                // Delegate to active panel
                let action = match self.active_panel {
                    Panel::FileTree => self.file_tree.handle_key_event(key),
                    Panel::GitStatus => self.git_status.handle_key_event(key),
                    Panel::CodeViewer => self.code_viewer.handle_key_event(key),
                };

                self.handle_action(action)?;
            }
        }
        Ok(())
    }

    fn handle_action(&mut self, action: Action) -> color_eyre::Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
            Action::FileSelected(path) => {
                if let Err(e) = self.code_viewer.load_file(path) {
                    eprintln!("Could not load file: {}", e);
                }
                self.active_panel = Panel::CodeViewer;
                // Refresh git status in case file changes affect it
                self.git_status.refresh();
            }
            _ => {}
        }
        Ok(())
    }
}
