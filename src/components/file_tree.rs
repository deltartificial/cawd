//! File tree explorer component for navigating directory structures.

use crate::action::Action;
use crate::components::Component;
use crossterm::event::{KeyCode, KeyEvent};
use devicons::FileIcon;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Represents a single node in the file tree.
///
/// Contains all information needed to display and interact with
/// a file or directory entry in the tree view.
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// Absolute path to the file or directory.
    pub path: PathBuf,
    /// Display name (filename without path).
    pub name: String,
    /// Whether this node is a directory.
    pub is_dir: bool,
    /// Icon character from devicons.
    pub icon: String,
    /// Color for the icon and filename.
    pub color: Color,
    /// Nesting depth in the tree (0 = root level).
    pub depth: usize,
    /// Whether this is the last sibling at its level.
    pub is_last: bool,
    /// Tracks which ancestors are last children (for tree line drawing).
    pub parent_is_last: Vec<bool>,
}

impl TreeNode {
    /// Creates a new tree node from a path.
    ///
    /// # Parameters
    ///
    /// * `path` - The absolute path to the file or directory.
    /// * `depth` - The nesting depth in the tree.
    /// * `is_last` - Whether this is the last sibling at its level.
    /// * `parent_is_last` - Vector tracking which ancestors are last children.
    ///
    /// # Returns
    ///
    /// A new `TreeNode` with icon and color determined from the filename.
    pub fn new(path: PathBuf, depth: usize, is_last: bool, parent_is_last: Vec<bool>) -> Self {
        let name = path
            .file_name()
            .map_or_else(|| path.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string());

        let is_dir = path.is_dir();

        let (icon, color) = if is_dir {
            ("\u{f07b}".to_string(), Color::Rgb(0xff, 0x7a, 0x5c))
        } else {
            let file_icon = FileIcon::from(&name);
            (file_icon.icon.to_string(), Self::devicon_color_to_ratatui(file_icon.color))
        };

        Self {
            path,
            name,
            is_dir,
            icon,
            color,
            depth,
            is_last,
            parent_is_last,
        }
    }

    /// Converts a hex color string to a ratatui Color.
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

/// File tree component for directory navigation.
///
/// Displays a hierarchical view of files and directories with
/// expand/collapse functionality, filtering, and file selection.
pub struct FileTree {
    root: PathBuf,
    nodes: Vec<TreeNode>,
    expanded: HashSet<PathBuf>,
    list_state: ListState,
    show_hidden: bool,
    search_query: String,
    search_mode: bool,
    filtered_indices: Vec<usize>,
}

impl FileTree {
    /// Creates a new file tree rooted at the given path.
    ///
    /// # Parameters
    ///
    /// * `path` - The root directory or file path.
    ///
    /// # Returns
    ///
    /// A new `FileTree` instance with the root directory expanded.
    pub fn new(path: PathBuf) -> color_eyre::Result<Self> {
        let root = if path.is_file() {
            path.parent().unwrap_or(&path).to_path_buf()
        } else {
            path
        };

        let mut expanded = HashSet::new();
        expanded.insert(root.clone());

        let mut tree = Self {
            root: root.clone(),
            nodes: Vec::new(),
            expanded,
            list_state: ListState::default(),
            show_hidden: false,
            search_query: String::new(),
            search_mode: false,
            filtered_indices: Vec::new(),
        };

        tree.rebuild_tree()?;
        tree.list_state.select(Some(0));

        Ok(tree)
    }

    /// Rebuilds the entire tree structure from the filesystem.
    fn rebuild_tree(&mut self) -> color_eyre::Result<()> {
        self.nodes.clear();
        self.build_tree_recursive(&self.root.clone(), 0, vec![])?;
        self.update_filtered_indices();
        Ok(())
    }

    /// Recursively builds tree nodes for a directory.
    fn build_tree_recursive(
        &mut self,
        dir: &Path,
        depth: usize,
        parent_is_last: Vec<bool>,
    ) -> color_eyre::Result<()> {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                if self.show_hidden {
                    true
                } else {
                    p.file_name()
                        .is_none_or(|n| !n.to_string_lossy().starts_with('.'))
                }
            })
            .collect();

        entries.sort_by(|a, b| {
            let a_is_dir = a.is_dir();
            let b_is_dir = b.is_dir();
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .cmp(&b.file_name().map(|n| n.to_string_lossy().to_lowercase())),
            }
        });

        let total = entries.len();
        for (i, path) in entries.into_iter().enumerate() {
            let is_last = i == total - 1;
            let node = TreeNode::new(path.clone(), depth, is_last, parent_is_last.clone());
            let is_dir = node.is_dir;
            self.nodes.push(node);

            if is_dir && self.expanded.contains(&path) {
                let mut new_parent_is_last = parent_is_last.clone();
                new_parent_is_last.push(is_last);
                self.build_tree_recursive(&path, depth + 1, new_parent_is_last)?;
            }
        }

        Ok(())
    }

    /// Updates the filtered indices based on the current search query.
    fn update_filtered_indices(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.nodes.len()).collect();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_indices = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }
    }

    /// Returns the number of visible items after filtering.
    fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }

    /// Returns the currently selected node, if any.
    fn selected_node(&self) -> Option<&TreeNode> {
        let selected = self.list_state.selected()?;
        let idx = *self.filtered_indices.get(selected)?;
        self.nodes.get(idx)
    }

    /// Moves selection up in the list.
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

    /// Moves selection down in the list.
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

    /// Toggles expansion of a directory or selects a file.
    fn toggle_expand(&mut self) -> Action {
        if let Some(node) = self.selected_node().cloned() {
            if node.is_dir {
                if self.expanded.contains(&node.path) {
                    self.expanded.remove(&node.path);
                } else {
                    self.expanded.insert(node.path);
                }
                self.rebuild_tree().ok(); // Intentionally ignore: previous state remains valid
                Action::None
            } else {
                Action::FileSelected(node.path)
            }
        } else {
            Action::None
        }
    }

    /// Collapses the current directory or navigates to parent.
    fn collapse_current(&mut self) {
        if let Some(node) = self.selected_node().cloned() {
            if node.is_dir && self.expanded.contains(&node.path) {
                self.expanded.remove(&node.path);
                self.rebuild_tree().ok(); // Intentionally ignore: previous state remains valid
            } else if node.depth > 0 {
                if let Some(parent) = node.path.parent() {
                    let parent_path = parent.to_path_buf();
                    if let Some(pos) = self.nodes.iter().position(|n| n.path == parent_path) {
                        if let Some(filtered_pos) = self.filtered_indices.iter().position(|&i| i == pos) {
                            self.list_state.select(Some(filtered_pos));
                        }
                    }
                }
            }
        }
    }

    /// Toggles visibility of hidden files (dotfiles).
    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild_tree().ok(); // Intentionally ignore: previous state remains valid
        self.list_state.select(Some(0));
    }

    /// Enters search/filter mode.
    pub fn enter_search_mode(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.update_filtered_indices();
    }

    /// Exits search/filter mode and clears the query.
    pub fn exit_search_mode(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    /// Appends a character to the search query.
    pub fn search_input(&mut self, c: char) {
        self.search_query.push(c);
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    /// Removes the last character from the search query.
    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.update_filtered_indices();
        self.list_state.select(Some(0));
    }

    /// Returns whether the component is in search mode.
    pub fn is_search_mode(&self) -> bool {
        self.search_mode
    }
}

impl Component for FileTree {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.exit_search_mode();
                    Action::ExitSearchMode
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                    self.toggle_expand()
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
                KeyCode::Left | KeyCode::Char('h') => {
                    self.collapse_current();
                    Action::None
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                    self.toggle_expand()
                }
                KeyCode::Char('.') => {
                    self.toggle_hidden();
                    Action::ToggleHidden
                }
                KeyCode::Char('/') => {
                    self.enter_search_mode();
                    Action::EnterSearchMode
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

        let root_name = self.root
            .file_name()
            .map_or_else(|| self.root.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string());
        let title = format!(" \u{f07b} {} ", root_name);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        if self.search_mode {
            let search_title = format!(" /{} ", self.search_query);
            block = block.title_bottom(Line::from(search_title).style(Style::default().fg(Color::Yellow)));
        }

        let tree_style = Style::default().fg(Color::DarkGray);

        let items: Vec<ListItem> = self
            .filtered_indices
            .iter()
            .filter_map(|&idx| self.nodes.get(idx))
            .map(|node| {
                let mut spans: Vec<Span> = Vec::with_capacity(node.depth + 4);

                for (i, &parent_last) in node.parent_is_last.iter().enumerate() {
                    if i < node.depth {
                        if parent_last {
                            spans.push(Span::styled("    ", tree_style));
                        } else {
                            spans.push(Span::styled("│   ", tree_style));
                        }
                    }
                }

                if node.depth > 0 {
                    if node.is_last {
                        spans.push(Span::styled("└── ", tree_style));
                    } else {
                        spans.push(Span::styled("├── ", tree_style));
                    }
                }

                if node.is_dir {
                    let indicator = if self.expanded.contains(&node.path) {
                        "\u{f0d7} "
                    } else {
                        "\u{f0da} "
                    };
                    spans.push(Span::styled(indicator, Style::default().fg(Color::Rgb(0xff, 0x7a, 0x5c))));
                }

                spans.push(Span::styled(
                    format!("{} ", node.icon),
                    Style::default().fg(node.color),
                ));

                let name_style = if node.is_dir {
                    Style::default().fg(Color::Rgb(0xff, 0x7a, 0x5c)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(node.color)
                };
                spans.push(Span::styled(&node.name, name_style));

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
