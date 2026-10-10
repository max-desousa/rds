//! Three-pane shell: directory tree, extension list, and a treemap.
//!
//! The treemap is the whole scan, the way WinDirStat draws one. Each block is
//! a file (or a directory too small to split), sized against the scan root.
//! The tree selection highlights that folder's region in place, so its area
//! stays its share of the cwd. `r` on a directory scans it again as the new
//! root. `..` is the first entry under that root, and `r` on it scans the
//! parent. Focusing an extension dims other types.

use std::collections::HashSet;

use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Cell, HighlightSpacing, Paragraph, Row, Table, TableState};

use crate::treemap;
use ratatui::{DefaultTerminal, Frame};

use crate::scan::{self, Scan};

const PALETTE: [Color; 16] = [
    Color::Rgb(0xe6, 0x4d, 0x4d),
    Color::Rgb(0x4d, 0xa6, 0xe6),
    Color::Rgb(0x4d, 0xe6, 0x7a),
    Color::Rgb(0xe6, 0xc2, 0x4d),
    Color::Rgb(0xb0, 0x4d, 0xe6),
    Color::Rgb(0xe6, 0x7a, 0x4d),
    Color::Rgb(0x4d, 0xe6, 0xe0),
    Color::Rgb(0xe6, 0x4d, 0xa6),
    Color::Rgb(0x8a, 0xe6, 0x4d),
    Color::Rgb(0x4d, 0x6e, 0xe6),
    Color::Rgb(0xe6, 0xe6, 0x4d),
    Color::Rgb(0x4d, 0xe6, 0xab),
    Color::Rgb(0xc4, 0x8a, 0x5a),
    Color::Rgb(0x9a, 0x9a, 0xe6),
    Color::Rgb(0xe6, 0x9a, 0xc4),
    Color::Rgb(0x7a, 0xc4, 0x7a),
];

const FOCUS_BG: Color = Color::Rgb(30, 70, 110);
const IDLE_BG: Color = Color::Rgb(45, 45, 55);
const DIR_COLOR: Color = Color::Rgb(0x3d, 0x4a, 0x63);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Extensions,
}

impl Focus {
    fn toggle(self) -> Self {
        match self {
            Self::Tree => Self::Extensions,
            Self::Extensions => Self::Tree,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Parent of the scan root. `r` scans that directory.
    Up,
    Node,
}

/// One visible directory-tree row. `path` is the child-index path from the root.
struct FlatRow {
    depth: usize,
    path: Vec<usize>,
    is_expanded: bool,
    has_children: bool,
    kind: RowKind,
}

pub(crate) struct App {
    scan: Scan,
    rows: Vec<FlatRow>,
    /// Expanded directories, keyed by child-index path. The empty path is the root.
    expanded: HashSet<Vec<usize>>,
    tree_selected: usize,
    ext_selected: usize,
    focus: Focus,
    tree_state: TableState,
    ext_state: TableState,
    should_quit: bool,
    /// Set by `r`. The run loop shows a rescan notice, then scans this path.
    pending_rescan: Option<std::path::PathBuf>,
    notice: Option<String>,
}

impl App {
    pub(crate) fn new(scan: Scan) -> Self {
        let mut app = Self {
            scan,
            rows: Vec::new(),
            expanded: HashSet::from([Vec::new()]),
            tree_selected: 0,
            ext_selected: 0,
            focus: Focus::Tree,
            tree_state: TableState::default(),
            ext_state: TableState::default(),
            should_quit: false,
            pending_rescan: None,
            notice: None,
        };
        app.rebuild_rows();
        app.sync_selection();
        app
    }

    pub(crate) fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            if let Some(path) = self.pending_rescan.take() {
                self.notice = Some(rescan_notice(&path));
                terminal.draw(|frame| self.draw(frame))?;
                let _ = self.rescan(&path);
                continue;
            }
            terminal.draw(|frame| self.draw(frame))?;
            if let Some(key) = event::read()?.as_key_press_event() {
                self.handle_key(key);
            }
        }
        Ok(())
    }

    /// Scan `path` as a new root, as if the tool had been started there.
    ///
    /// On failure the current tree stays and [`Self::notice`] explains why.
    fn rescan(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        match scan::scan(path) {
            Ok(scan) => {
                *self = Self::new(scan);
                Ok(())
            }
            Err(error) => {
                self.notice = Some(format!("Rescan failed: {error}"));
                Err(error)
            }
        }
    }

    /// Directory `r` would scan, if the tree selection is a directory or `..`.
    fn rescan_target(&self) -> Option<std::path::PathBuf> {
        let row = self.rows.get(self.tree_selected)?;
        if row.kind == RowKind::Up {
            return Some(self.scan.root_path.join(".."));
        }
        let node = self.scan.node(&row.path);
        if !node.is_dir {
            return None;
        }
        Some(self.filesystem_path(&row.path))
    }

    fn filesystem_path(&self, index_path: &[usize]) -> std::path::PathBuf {
        let mut path = self.scan.root_path.clone();
        let mut node = &self.scan.root;
        for &index in index_path {
            node = &node.children[index];
            path.push(&node.name);
        }
        path
    }

    fn request_rescan(&mut self) {
        let Some(path) = self.rescan_target() else {
            return;
        };
        self.notice = None;
        self.pending_rescan = Some(path);
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab | KeyCode::BackTab => self.focus = self.focus.toggle(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.move_to_end(true),
            KeyCode::End => self.move_to_end(false),
            KeyCode::Right | KeyCode::Char('l') => self.on_right(),
            KeyCode::Left | KeyCode::Char('h') => self.on_left(),
            KeyCode::Enter => self.on_enter(),
            KeyCode::Char('r') | KeyCode::Char('R')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.request_rescan();
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Tree => self.move_index(self.rows.len(), true, delta),
            Focus::Extensions => self.move_index(self.scan.extensions.len(), false, delta),
        }
    }

    fn move_to_end(&mut self, first: bool) {
        let (len, tree) = match self.focus {
            Focus::Tree => (self.rows.len(), true),
            Focus::Extensions => (self.scan.extensions.len(), false),
        };
        if len == 0 {
            return;
        }
        let index = if first { 0 } else { len - 1 };
        self.set_index(tree, index);
    }

    fn move_index(&mut self, len: usize, tree: bool, delta: isize) {
        if len == 0 {
            return;
        }
        let current = if tree {
            self.tree_selected
        } else {
            self.ext_selected
        } as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.set_index(tree, next);
    }

    fn set_index(&mut self, tree: bool, index: usize) {
        if tree {
            self.tree_selected = index;
        } else {
            self.ext_selected = index;
        }
        self.sync_selection();
    }

    fn on_right(&mut self) {
        if self.focus != Focus::Tree {
            return;
        }
        let Some(path) = self.selected_path() else {
            return;
        };
        if !self.can_expand(&path) {
            return;
        }
        if self.expanded.insert(path.clone()) {
            self.rebuild_rows();
        } else {
            let mut child = path;
            child.push(0);
            self.select_path(&child);
        }
    }

    fn on_left(&mut self) {
        if self.focus != Focus::Tree {
            return;
        }
        if self
            .rows
            .get(self.tree_selected)
            .is_some_and(|row| row.kind == RowKind::Up)
        {
            self.select_path(&[]);
            return;
        }
        let Some(path) = self.selected_path() else {
            return;
        };
        if self.expanded.contains(&path) && self.can_expand(&path) {
            self.expanded.remove(&path);
            self.rebuild_rows();
            return;
        }
        if path.is_empty() {
            return;
        }
        let mut parent = path;
        parent.pop();
        self.select_path(&parent);
    }

    fn on_enter(&mut self) {
        if self.focus != Focus::Tree {
            return;
        }
        let Some(path) = self.selected_path() else {
            return;
        };
        if !self.can_expand(&path) {
            return;
        }
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild_rows();
    }

    fn selected_path(&self) -> Option<Vec<usize>> {
        let row = self.rows.get(self.tree_selected)?;
        if row.kind == RowKind::Up {
            None
        } else {
            Some(row.path.clone())
        }
    }

    fn can_expand(&self, path: &[usize]) -> bool {
        let node = self.scan.node(path);
        node.is_dir && !node.children.is_empty()
    }

    fn select_path(&mut self, path: &[usize]) {
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| row.kind == RowKind::Node && row.path == path)
        {
            self.tree_selected = index;
            self.sync_selection();
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        flatten(
            &self.scan.root,
            Vec::new(),
            0,
            &self.expanded,
            &mut self.rows,
        );
        if self.rows.is_empty() {
            self.tree_selected = 0;
        } else if self.tree_selected >= self.rows.len() {
            self.tree_selected = self.rows.len() - 1;
        }
        self.sync_selection();
    }

    fn sync_selection(&mut self) {
        if self.rows.is_empty() {
            self.tree_state.select(None);
        } else {
            self.tree_state.select(Some(self.tree_selected));
        }
        if self.scan.extensions.is_empty() {
            self.ext_state.select(None);
            return;
        }
        if self.ext_selected >= self.scan.extensions.len() {
            self.ext_selected = self.scan.extensions.len() - 1;
        }
        self.ext_state.select(Some(self.ext_selected));
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [top, map, status] = Layout::vertical([
            Constraint::Fill(3),
            Constraint::Fill(2),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [tree, extensions] =
            Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(top);

        self.draw_tree(frame, tree);
        self.draw_extensions(frame, extensions);
        self.draw_treemap(frame, map);
        self.draw_status(frame, status);
    }

    fn draw_tree(&mut self, frame: &mut Frame, area: Rect) {
        let total = self.scan.root.size;
        let rows: Vec<Row> = self
            .rows
            .iter()
            .map(|row| {
                if row.kind == RowKind::Up {
                    let name = format!("{}..", "  ".repeat(row.depth));
                    return Row::new([
                        Cell::from(name),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                    ]);
                }
                let node = self.scan.node(&row.path);
                let marker = if !node.is_dir || !row.has_children {
                    "  "
                } else if row.is_expanded {
                    "▾ "
                } else {
                    "▸ "
                };
                let name = format!("{}{marker}{}", "  ".repeat(row.depth), node.name);
                let files = if node.is_dir {
                    node.file_count.to_string()
                } else {
                    String::new()
                };
                Row::new([
                    Cell::from(name),
                    Cell::from(Text::from(scan::format_size(node.size)).right_aligned()),
                    Cell::from(Text::from(scan::format_percent(node.size, total)).right_aligned()),
                    Cell::from(Text::from(files).right_aligned()),
                ])
            })
            .collect();

        let focused = self.focus == Focus::Tree;
        let table = Table::new(
            rows,
            [
                Constraint::Fill(1),
                Constraint::Length(8),
                Constraint::Length(6),
                Constraint::Length(7),
            ],
        )
        .header(header(&["Name", "Size", "%", "Files"], 1))
        .block(pane_block("Directory", focused))
        .row_highlight_style(highlight(focused))
        .highlight_symbol(">")
        .highlight_spacing(HighlightSpacing::Always);
        frame.render_stateful_widget(table, area, &mut self.tree_state);
    }

    fn draw_extensions(&mut self, frame: &mut Frame, area: Rect) {
        let total = self.scan.root.size;
        let rows: Vec<Row> = self
            .scan
            .extensions
            .iter()
            .map(|extension| {
                let color = PALETTE[extension.color_index % PALETTE.len()];
                Row::new([
                    Cell::from("██").fg(color),
                    Cell::from(extension_label(&extension.name)),
                    Cell::from(Text::from(scan::format_size(extension.size)).right_aligned()),
                    Cell::from(
                        Text::from(scan::format_percent(extension.size, total)).right_aligned(),
                    ),
                    Cell::from(Text::from(extension.count.to_string()).right_aligned()),
                ])
            })
            .collect();

        let focused = self.focus == Focus::Extensions;
        let table = Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(8),
                Constraint::Length(6),
                Constraint::Length(7),
            ],
        )
        .header(header(&["", "Ext", "Size", "%", "Files"], 2))
        .block(pane_block("Extensions", focused))
        .row_highlight_style(highlight(focused))
        .highlight_symbol(">")
        .highlight_spacing(HighlightSpacing::Always);
        frame.render_stateful_widget(table, area, &mut self.ext_state);
    }

    fn draw_treemap(&self, frame: &mut Frame, area: Rect) {
        let on_up = self
            .rows
            .get(self.tree_selected)
            .is_some_and(|row| row.kind == RowKind::Up);
        let selected = if on_up {
            Vec::new()
        } else {
            self.selected_path().unwrap_or_default()
        };
        let title = if on_up {
            format!(
                "Treemap  ..  r rescans {}",
                self.scan.root_path.join("..").display()
            )
        } else {
            let node = self.scan.node(&selected);
            format!(
                "Treemap  {}  {}  {}",
                self.display_path(&selected),
                scan::format_size(node.size),
                scan::format_percent(node.size, self.scan.root.size),
            )
        };
        let block = pane_block(&title, false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let extension_filter = self.emphasized_extension().map(str::to_string);
        let mut paints = Vec::new();
        let mut highlight = None;
        let mut path = Vec::new();
        let state = DrawState {
            scan: &self.scan,
            selected: &selected,
            filter: extension_filter.as_deref(),
        };
        descend(
            &state,
            &self.scan.root,
            &mut path,
            inner,
            &mut paints,
            &mut highlight,
        );
        if paints.is_empty() {
            frame.render_widget(Paragraph::new("empty").centered(), inner);
            return;
        }
        let buf = frame.buffer_mut();
        for paint in &paints {
            paint_tile(buf, paint.rect, &paint.label, paint.color, paint.selected);
        }
        // The root already fills the pane. A frame around it would retrace the
        // border. A nested selection gets a frame so that folder's share of
        // the cwd stays visible beside the rest of the scan.
        if !selected.is_empty()
            && let Some(rect) = highlight
        {
            stroke_highlight(buf, rect);
        }
    }

    fn emphasized_extension(&self) -> Option<&str> {
        if self.focus != Focus::Extensions {
            return None;
        }
        self.scan
            .extensions
            .get(self.ext_selected)
            .map(|extension| extension.name.as_str())
    }

    fn display_path(&self, path: &[usize]) -> String {
        let mut parts = vec![self.scan.root.name.as_str()];
        let mut node = &self.scan.root;
        for &index in path {
            node = &node.children[index];
            parts.push(node.name.as_str());
        }
        parts.join("/")
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let unread = if self.scan.errors == 0 {
            String::new()
        } else {
            format!("   {} unreadable", self.scan.errors)
        };
        let text = if let Some(notice) = &self.notice {
            format!(" {notice}    q quit")
        } else {
            format!(
                " j/k move  h/l fold  r rescan  tab pane  q quit    {}    {}    {} files{unread}",
                self.scan.root_path.display(),
                scan::format_size(self.scan.root.size),
                self.scan.root.file_count,
            )
        };
        let style = if self.notice.is_some() {
            Style::new().bg(Color::Rgb(90, 70, 20)).fg(Color::Yellow)
        } else {
            Style::new().bg(Color::Rgb(28, 28, 32)).fg(Color::Gray)
        };
        frame.render_widget(Line::from(text).style(style), area);
    }
}

struct DrawState<'a> {
    scan: &'a Scan,
    selected: &'a [usize],
    filter: Option<&'a str>,
}

struct TilePaint {
    rect: Rect,
    label: String,
    color: Color,
    selected: bool,
}

/// Lay the whole scan into `area`. Directory rectangles are split into their
/// children, so a folder keeps the share of the cwd its bytes represent.
fn descend(
    state: &DrawState<'_>,
    node: &scan::Node,
    path: &mut Vec<usize>,
    area: Rect,
    paints: &mut Vec<TilePaint>,
    highlight: &mut Option<Rect>,
) {
    if area.width == 0 || area.height == 0 || node.size == 0 {
        return;
    }
    if path.as_slice() == state.selected {
        *highlight = Some(area);
    }
    if node.is_dir
        && area.width >= 2
        && area.height >= 2
        && node.children.iter().any(|child| child.size > 0)
    {
        let weights: Vec<u64> = node.children.iter().map(|child| child.size).collect();
        let tiles = treemap::layout(&weights, area);
        for (index, tile) in tiles.into_iter().enumerate() {
            if tile.width == 0 || tile.height == 0 {
                continue;
            }
            path.push(index);
            descend(state, &node.children[index], path, tile, paints, highlight);
            path.pop();
        }
        return;
    }

    let base = node_color(state.scan, node);
    let in_highlight = !state.selected.is_empty() && path.starts_with(state.selected);
    let mut color = base;
    if !in_highlight && state.filter.is_some() && !extension_matches(node, state.filter) {
        color = scale_color(base, 0.35, 0);
    }
    if in_highlight {
        color = scale_color(base, 1.0, 48);
    }
    let label = if area.width >= 3 {
        tile_label(node, area.width)
    } else {
        String::new()
    };
    paints.push(TilePaint {
        rect: area,
        label,
        color,
        selected: in_highlight,
    });
}

fn node_color(scan: &Scan, node: &scan::Node) -> Color {
    if node.is_dir {
        return DIR_COLOR;
    }
    let name = node.extension.as_deref().unwrap_or("(none)");
    scan.extensions
        .iter()
        .find(|extension| extension.name == name)
        .map(|extension| PALETTE[extension.color_index % PALETTE.len()])
        .unwrap_or(Color::Gray)
}

fn stroke_highlight(buf: &mut ratatui::buffer::Buffer, rect: Rect) {
    if rect.width < 3 || rect.height < 3 {
        return;
    }
    let style = Style::new()
        .bg(Color::Rgb(0xff, 0xdc, 0x50))
        .fg(Color::Black);
    for x in rect.x..rect.right() {
        buf[(x, rect.y)].set_style(style);
        buf[(x, rect.bottom() - 1)].set_style(style);
    }
    for y in rect.y..rect.bottom() {
        buf[(rect.x, y)].set_style(style);
        buf[(rect.right() - 1, y)].set_style(style);
    }
}

fn paint_tile(
    buf: &mut ratatui::buffer::Buffer,
    tile: Rect,
    label: &str,
    color: Color,
    selected: bool,
) {
    let fg = label_fg(color);
    buf.set_style(tile, Style::new().bg(color).fg(fg));
    if !selected && tile.width > 1 && tile.height > 1 {
        let edge = scale_color(color, 0.55, 0);
        let right = Rect {
            x: tile.right() - 1,
            y: tile.y,
            width: 1,
            height: tile.height,
        };
        let bottom = Rect {
            x: tile.x,
            y: tile.bottom() - 1,
            width: tile.width,
            height: 1,
        };
        buf.set_style(right, Style::new().bg(edge));
        buf.set_style(bottom, Style::new().bg(edge));
    }
    if tile.width >= 3 && tile.height >= 1 && !label.is_empty() {
        // Keep the name inside the selection frame when that frame is drawn.
        let (x, y, width) = if selected && tile.width >= 4 && tile.height >= 3 {
            (tile.x + 1, tile.y + 1, tile.width.saturating_sub(2))
        } else if tile.height > 1 {
            (tile.x, tile.y, tile.width.saturating_sub(1))
        } else {
            (tile.x, tile.y, tile.width)
        };
        buf.set_stringn(
            x,
            y,
            label,
            width as usize,
            Style::new().bg(color).fg(fg).bold(),
        );
    }
}

fn tile_label(node: &scan::Node, width: u16) -> String {
    let name = if node.is_dir {
        format!("{}/", node.name)
    } else {
        node.name.clone()
    };
    let with_size = format!("{name}  {}", scan::format_size(node.size));
    if with_size.chars().count() <= usize::from(width) {
        with_size
    } else {
        name
    }
}

fn rescan_notice(path: &std::path::Path) -> String {
    format!("Rescanning {}…", path.display())
}

fn extension_matches(node: &scan::Node, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if node.is_dir {
        return false;
    }
    node.extension.as_deref().unwrap_or("(none)") == filter
}

fn scale_color(color: Color, factor: f64, add: u8) -> Color {
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    let channel = |value: u8| -> u8 {
        let scaled = (f64::from(value) * factor).round() as u16 + u16::from(add);
        scaled.min(255) as u8
    };
    Color::Rgb(channel(red), channel(green), channel(blue))
}

fn label_fg(background: Color) -> Color {
    let Color::Rgb(red, green, blue) = background else {
        return Color::White;
    };
    let luminance = 0.299 * f64::from(red) + 0.587 * f64::from(green) + 0.114 * f64::from(blue);
    if luminance > 150.0 {
        Color::Black
    } else {
        Color::White
    }
}

fn flatten(
    node: &scan::Node,
    path: Vec<usize>,
    depth: usize,
    expanded: &HashSet<Vec<usize>>,
    out: &mut Vec<FlatRow>,
) {
    let has_children = node.is_dir && (!node.children.is_empty() || path.is_empty());
    let is_expanded = has_children && expanded.contains(&path);
    out.push(FlatRow {
        depth,
        path: path.clone(),
        is_expanded,
        has_children,
        kind: RowKind::Node,
    });
    if is_expanded {
        // `..` is only on the scan root, so `r` walks up one directory at a time.
        if path.is_empty() {
            out.push(FlatRow {
                depth: depth + 1,
                path: Vec::new(),
                is_expanded: false,
                has_children: false,
                kind: RowKind::Up,
            });
        }
        for (index, child) in node.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(index);
            flatten(child, child_path, depth + 1, expanded, out);
        }
    }
}

fn header(labels: &[&str], left_aligned: usize) -> Row<'static> {
    let cells: Vec<Cell> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let text = Text::from((*label).to_string());
            if index < left_aligned {
                Cell::from(text)
            } else {
                Cell::from(text.right_aligned())
            }
        })
        .collect();
    Row::new(cells).style(Style::new().bold().fg(Color::Gray))
}

fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let border = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Block::bordered()
        .title(format!(" {title} "))
        .border_style(border)
}

fn highlight(focused: bool) -> Style {
    // Background only, so extension swatches keep their foreground color.
    if focused {
        Style::new().bg(FOCUS_BG)
    } else {
        Style::new().bg(IDLE_BG)
    }
}

fn extension_label(name: &str) -> String {
    if name == "(none)" {
        name.to_string()
    } else {
        format!(".{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::CellWidth;
    use ratatui::style::Color;
    use std::fs;
    use std::path::PathBuf;

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "foo-app-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sample() -> (Scratch, App) {
        let scratch = Scratch::new();
        fs::write(scratch.path.join("big.dat"), vec![0_u8; 30]).unwrap();
        fs::write(scratch.path.join("small.txt"), vec![0_u8; 10]).unwrap();
        let sub = scratch.path.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("tiny.txt"), vec![0_u8; 5]).unwrap();
        let app = App::new(scan(&scratch.path).unwrap());
        (scratch, app)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn row_name(app: &App, index: usize) -> String {
        let row = &app.rows[index];
        if row.kind == RowKind::Up {
            "..".to_string()
        } else {
            app.scan.node(&row.path).name.clone()
        }
    }

    fn view(app: &mut App) -> String {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut lines = String::new();
        for y in 0..buffer.area.height {
            let mut skip = 0u16;
            for x in 0..buffer.area.width {
                if skip == 0 {
                    let cell = &buffer[(x, y)];
                    lines.push_str(cell.symbol());
                    skip = cell.cell_width().saturating_sub(1);
                } else {
                    skip -= 1;
                }
            }
            lines.push('\n');
        }
        lines
    }

    #[test]
    fn root_starts_expanded_with_children_sorted_by_size() {
        let (_scratch, app) = sample();
        let names: Vec<String> = (0..app.rows.len())
            .map(|index| row_name(&app, index))
            .collect();
        assert_eq!(names[0], app.scan.root.name);
        assert_eq!(&names[1..], ["..", "big.dat", "small.txt", "sub"]);
        assert_eq!(app.rows[1].kind, RowKind::Up);
        assert_eq!(app.tree_selected, 0);
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn keys_move_fold_and_switch_panes() {
        let (_scratch, mut app) = sample();

        press(&mut app, KeyCode::Down);
        assert_eq!(app.rows[app.tree_selected].kind, RowKind::Up);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.rows[app.tree_selected].kind, RowKind::Up);

        press(&mut app, KeyCode::Down);
        assert_eq!(row_name(&app, app.tree_selected), "big.dat");

        press(&mut app, KeyCode::Left);
        assert_eq!(app.tree_selected, 0);

        press(&mut app, KeyCode::Left);
        assert_eq!(app.rows.len(), 1);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.rows.len(), 5);
        assert_eq!(app.rows[1].kind, RowKind::Up);

        // The root is already expanded, so right enters its first real child.
        press(&mut app, KeyCode::Right);
        assert_eq!(row_name(&app, app.tree_selected), "big.dat");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.tree_selected, 0);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(row_name(&app, app.tree_selected), "sub");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.scan.node(&app.rows[app.tree_selected + 1].path).name,
            "tiny.txt"
        );

        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Extensions);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.ext_selected, 1);
        assert_eq!(app.scan.extensions[app.ext_selected].name, "txt");

        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn render_shows_three_panes_and_selection() {
        let (_scratch, mut app) = sample();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        let rendered = view(&mut app);

        assert!(rendered.contains("Directory"), "{rendered}");
        assert!(rendered.contains("Extensions"), "{rendered}");
        assert!(rendered.contains("Treemap"), "{rendered}");
        assert!(rendered.contains("big.dat"), "{rendered}");
        assert!(rendered.contains(".dat"), "{rendered}");
        assert!(rendered.contains(".txt"), "{rendered}");
        assert!(!rendered.contains("Not drawn yet."), "{rendered}");
        assert!(rendered.contains(".."), "{rendered}");
        assert!(rendered.contains("r rescan"), "{rendered}");
        assert!(rendered.contains("q quit"), "{rendered}");
        let map = treemap_section(&rendered);
        assert!(map.contains("big.dat"), "{map}");
        assert!(map.contains("66.7%"), "{map}");
        assert!(map.contains("small.txt"), "{map}");

        let ext_rows: Vec<&str> = rendered.lines().filter(|line| line.contains('█')).collect();
        assert!(
            ext_rows.iter().any(|line| line.contains(".dat")),
            "extension column clipped .dat: {rendered}"
        );
        assert!(
            ext_rows.iter().any(|line| line.contains(".txt")),
            "extension column clipped .txt: {rendered}"
        );

        let swatches = swatch_colors(&mut app);
        assert!(
            swatches.contains(&PALETTE[0]),
            "missing color for the largest extension: {swatches:?}"
        );
        assert!(
            swatches.contains(&PALETTE[1]),
            "missing color for the second extension: {swatches:?}"
        );
    }

    #[test]
    fn treemap_follows_the_selected_directory_and_extension() {
        let (_scratch, mut app) = sample();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        let selected = scale_color(PALETTE[0], 1.0, 48);
        let hot_dat = count_bg(&mut app, selected);
        assert!(hot_dat > 20, "highlighted file has no area");
        assert_eq!(count_bg(&mut app, PALETTE[0]), 0);
        // The rest of the cwd stays on screen, so the file does not fill the map.
        assert!(count_bg(&mut app, PALETTE[1]) > 0);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.scan.node(&app.rows[app.tree_selected].path).name, "sub");
        let rendered = view(&mut app);
        let map = treemap_section(&rendered);
        assert!(map.contains("11.1%"), "{map}");
        assert!(map.contains("big.dat"), "{map}");
        let hot_txt = count_bg(&mut app, scale_color(PALETTE[1], 1.0, 48));
        let plain_dat = count_bg(&mut app, PALETTE[0]);
        assert!(hot_txt > 0, "highlighted folder has no blocks");
        assert!(
            plain_dat > hot_txt,
            "folder filled the map instead of its share of cwd: dat {plain_dat}, highlighted {hot_txt}"
        );

        let (_scratch, mut app) = sample();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Extensions);
        assert_eq!(count_bg(&mut app, PALETTE[1]), 0);
        assert!(count_bg(&mut app, PALETTE[0]) > 0);
    }

    #[test]
    fn rescan_replaces_the_root_and_dotdot_walks_back() {
        let scratch = Scratch::new();
        fs::write(scratch.path.join("keep.txt"), vec![0_u8; 4]).unwrap();
        let mid = scratch.path.join("mid");
        fs::create_dir(&mid).unwrap();
        fs::write(mid.join("sibling.txt"), vec![0_u8; 8]).unwrap();
        let inner = mid.join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("file.dat"), vec![0_u8; 20]).unwrap();

        let mut app = App::new(scan(&inner).unwrap());
        assert_eq!(app.scan.root.name, "inner");
        assert_eq!(row_name(&app, 1), "..");

        press(&mut app, KeyCode::Char('r'));
        let target = app
            .pending_rescan
            .clone()
            .expect("r on the root queues a rescan");
        assert_eq!(
            target.canonicalize().unwrap(),
            inner.canonicalize().unwrap()
        );
        app.notice = Some(rescan_notice(&target));
        let rendered = view(&mut app);
        assert!(rendered.contains("Rescanning"), "{rendered}");
        assert!(rendered.contains("inner"), "{rendered}");
        assert_eq!(status_style(&mut app).0, Color::Yellow);
        app.rescan(&target).unwrap();
        assert!(app.notice.is_none());
        assert!(app.pending_rescan.is_none());
        assert_eq!(app.scan.root.name, "inner");
        assert!(!names_of(&app).contains(&"sibling.txt".to_string()));

        press(&mut app, KeyCode::Down);
        assert_eq!(app.rows[app.tree_selected].kind, RowKind::Up);
        let on_up = view(&mut app);
        let map = treemap_section(&on_up);
        assert!(map.contains("r rescans"), "{map}");
        assert!(map.contains("file.dat"), "{map}");
        press(&mut app, KeyCode::Char('R'));
        let parent = app
            .pending_rescan
            .clone()
            .expect("r on .. queues the parent");
        assert_eq!(parent.canonicalize().unwrap(), mid.canonicalize().unwrap());
        app.rescan(&parent).unwrap();
        assert_eq!(app.scan.root_path, mid.canonicalize().unwrap());
        assert!(names_of(&app).contains(&"sibling.txt".to_string()));
        assert!(names_of(&app).contains(&"inner".to_string()));
        assert_eq!(row_name(&app, 1), "..");

        let inner_row = (0..app.rows.len())
            .find(|index| row_name(&app, *index) == "inner")
            .expect("inner is listed under mid");
        app.tree_selected = inner_row;
        app.sync_selection();
        press(&mut app, KeyCode::Char('r'));
        let into = app
            .pending_rescan
            .clone()
            .expect("r on a directory queues it");
        assert_eq!(into.canonicalize().unwrap(), inner.canonicalize().unwrap());
        app.rescan(&into).unwrap();
        assert_eq!(app.scan.root.name, "inner");
        assert!(!names_of(&app).contains(&"sibling.txt".to_string()));

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('r'));
        app.rescan(&app.pending_rescan.clone().unwrap()).unwrap();
        assert_eq!(app.scan.root_path, mid.canonicalize().unwrap());
        assert!(names_of(&app).contains(&"sibling.txt".to_string()));

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('r'));
        app.rescan(&app.pending_rescan.clone().unwrap()).unwrap();
        assert_eq!(app.scan.root_path, scratch.path.canonicalize().unwrap());
        assert!(names_of(&app).contains(&"keep.txt".to_string()));
        assert!(names_of(&app).contains(&"mid".to_string()));
        assert!(!names_of(&app).contains(&"file.dat".to_string()));
        assert_eq!(row_name(&app, 1), "..");
    }

    #[test]
    fn rescan_ignores_files_and_a_failed_rescan_keeps_the_tree() {
        let (_scratch, mut app) = sample();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(row_name(&app, app.tree_selected), "big.dat");
        press(&mut app, KeyCode::Char('r'));
        assert!(app.pending_rescan.is_none());
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.pending_rescan.is_none());

        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.tree_selected, 0);
        let missing = app.scan.root_path.join("missing-dir");
        let error = app.rescan(&missing).unwrap_err();
        let notice = app.notice.clone().expect("failure is reported");
        assert!(notice.contains("Rescan failed"), "{notice}");
        assert!(notice.contains(&error.to_string()), "{notice}");
        assert_eq!(row_name(&app, 2), "big.dat");
        let rendered = view(&mut app);
        assert!(rendered.contains("Rescan failed"), "{rendered}");
        assert!(rendered.contains("big.dat"), "{rendered}");
    }

    fn names_of(app: &App) -> Vec<String> {
        (0..app.rows.len())
            .map(|index| row_name(app, index))
            .collect()
    }

    fn status_style(app: &mut App) -> (Color, Color) {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let cell = &buffer[(0, buffer.area.height - 1)];
        (cell.fg, cell.bg)
    }

    fn treemap_section(rendered: &str) -> String {
        let lines: Vec<&str> = rendered.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.contains("Treemap"))
            .unwrap_or(0);
        lines[start..].join("\n")
    }

    fn count_bg(app: &mut App, color: Color) -> usize {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut count = 0;
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if buffer[(x, y)].bg == color {
                    count += 1;
                }
            }
        }
        count
    }

    fn swatch_colors(app: &mut App) -> Vec<Color> {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut colors = Vec::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol().contains('█') {
                    colors.push(cell.fg);
                }
            }
        }
        colors
    }
}
