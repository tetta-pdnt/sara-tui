use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use percent_encoding::percent_decode_str;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use unicode_segmentation::UnicodeSegmentation;

use crate::markdown::{render_mermaid, MarkdownPreview, MermaidDiagram};
use crate::sara::{Item, ItemType, Sara, SearchHit, Traversal, Validation};
use crate::tree::{Row, RowKind, Tree};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Items,
    Body,
    Links,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyTab {
    #[default]
    Body,
    Upstream,
    Downstream,
}

impl BodyTab {
    pub const ALL: [Self; 3] = [Self::Body, Self::Upstream, Self::Downstream];

    pub fn label(self) -> &'static str {
        self.traversal().map_or("Body", Traversal::label)
    }

    pub fn traversal(self) -> Option<Traversal> {
        match self {
            Self::Body => None,
            Self::Upstream => Some(Traversal::Upstream),
            Self::Downstream => Some(Traversal::Downstream),
        }
    }

    fn cycle(self, reverse: bool) -> Self {
        match (self, reverse) {
            (Self::Body, false) | (Self::Downstream, true) => Self::Upstream,
            (Self::Upstream, false) | (Self::Body, true) => Self::Downstream,
            (Self::Downstream, false) | (Self::Upstream, true) => Self::Body,
        }
    }
}

pub enum Overlay {
    Help,
    Validation(Validation),
    Error(String),
}

pub struct TraceView {
    pub diagram: MermaidDiagram,
    pub width: usize,
    pub left: Option<usize>,
    pub scroll: Scroll,
    pub visible_nodes: Vec<(Rect, usize)>,
}

impl TraceView {
    fn handle_key(&mut self, code: KeyCode) {
        self.scroll.handle_key(code);
        if let Some(left) = &mut self.left {
            match code {
                KeyCode::Left => *left = left.saturating_sub(8),
                KeyCode::Right => *left += 8,
                _ => {}
            }
        }
    }
}

pub enum Action {
    Continue,
    Quit,
    Edit,
}

#[derive(Default)]
pub struct Scroll {
    pub offset: u16,
    pub limit: u16,
    pub height: u16,
}

impl Scroll {
    pub fn set_viewport(&mut self, content_height: usize, height: u16) {
        let limit = content_height.saturating_sub(usize::from(height));
        self.height = height;
        self.limit = u16::try_from(limit).unwrap_or(u16::MAX);
        self.offset = self.offset.min(self.limit);
    }

    fn handle_key(&mut self, code: KeyCode) {
        let page = self.height.saturating_sub(1).max(1);
        self.offset = match code {
            KeyCode::Up => self.offset.saturating_sub(1),
            KeyCode::Down => self.offset.saturating_add(1),
            KeyCode::PageUp => self.offset.saturating_sub(page),
            KeyCode::PageDown => self.offset.saturating_add(page),
            KeyCode::Home => 0,
            KeyCode::End => self.limit,
            _ => self.offset,
        }
        .min(self.limit);
    }
}

pub struct App {
    pub sara: Sara,
    pub query: String,
    pub search: Option<String>,
    pub tree: Tree,
    pub item: Option<Item>,
    pub items_state: ListState,
    pub items_height: u16,
    pub links_state: ListState,
    pub pane: Pane,
    pub body_tab: BodyTab,
    pub markdown: MarkdownPreview,
    pub body_scroll: Scroll,
    pub body_area: Rect,
    pub body_links: Vec<(Rect, usize)>,
    pub body_anchor: Option<String>,
    pub overlay: Option<Overlay>,
    pub overlay_scroll: Scroll,
    traces: HashMap<Traversal, std::result::Result<TraceView, String>>,
    pending_g: bool,
    history: Vec<(String, Pane, u16)>,
}

impl App {
    pub fn new(sara: Sara) -> Result<Self> {
        let mut app = Self {
            sara,
            query: String::new(),
            search: None,
            tree: Tree::default(),
            item: None,
            items_state: ListState::default(),
            items_height: 0,
            links_state: ListState::default(),
            pane: Pane::Items,
            body_tab: BodyTab::default(),
            markdown: MarkdownPreview::default(),
            body_scroll: Scroll::default(),
            body_area: Rect::ZERO,
            body_links: Vec::new(),
            body_anchor: None,
            overlay: None,
            overlay_scroll: Scroll::default(),
            traces: HashMap::new(),
            pending_g: false,
            history: Vec::new(),
        };
        app.load_tree(String::new(), false)?;
        Ok(app)
    }

    fn load_tree(&mut self, query: String, refresh: bool) -> Result<()> {
        let item_types = self.sara.item_types()?;
        let hits = self.sara.search(&query)?;
        let (tree, selected) = self.build_tree(hits, &item_types, &query, refresh);
        let item = selected
            .and_then(|position| tree.row_at(position))
            .and_then(document_id)
            .map(|id| self.sara.item(id))
            .transpose()?;

        self.query = query;
        self.tree = tree;
        self.items_state = ListState::default().with_selected(selected);
        self.pane = Pane::Items;
        self.history.clear();
        self.set_item(item);
        Ok(())
    }

    fn build_tree(
        &self,
        hits: Vec<SearchHit>,
        item_types: &[ItemType],
        query: &str,
        refresh: bool,
    ) -> (Tree, Option<usize>) {
        let best_match = hits
            .first()
            .filter(|_| !query.is_empty())
            .map(|hit| hit.item.id.clone());
        let mut tree = Tree::new(hits, item_types);
        let mut selected = None;
        if refresh {
            tree.preserve_folds(&self.tree);
            selected = match self.selected_row().map(|row| &row.kind) {
                Some(RowKind::Document(item)) => tree.reveal(&item.id),
                Some(RowKind::Group { type_id, .. }) => tree.group_position(type_id),
                None => None,
            };
        }
        let selected = selected
            .or_else(|| best_match.as_deref().and_then(|id| tree.reveal(id)))
            .or_else(|| tree.first_document());
        (tree, selected)
    }

    fn set_item(&mut self, item: Option<Item>) {
        self.item = item;
        self.markdown = MarkdownPreview::default();
        self.traces.clear();
        self.links_state = ListState::default().with_selected((self.link_count() > 0).then_some(0));
        self.body_scroll = Scroll::default();
        self.body_anchor = None;
    }

    fn selected_row(&self) -> Option<&Row> {
        self.items_state
            .selected()
            .and_then(|position| self.tree.row_at(position))
    }

    pub fn sidebar_matches_preview(&self) -> bool {
        self.selected_row().and_then(document_id) == self.item.as_ref().map(|item| item.id.as_str())
    }

    fn select_row(&mut self, position: usize) -> Result<()> {
        let id = self.tree.row_at(position).and_then(document_id);
        if id != self.item.as_ref().map(|item| item.id.as_str()) {
            let item = id.map(|id| self.sara.item(id)).transpose()?;
            self.history.clear();
            self.set_item(item);
        }
        self.items_state.select(Some(position));
        Ok(())
    }

    fn focus(&mut self, pane: Pane) -> Result<()> {
        if pane == Pane::Items {
            if let Some(position) = self.items_state.selected() {
                self.select_row(position)?;
            } else {
                self.set_item(None);
                self.history.clear();
            }
        }
        self.pane = pane;
        Ok(())
    }

    fn move_in_tree(&mut self, code: KeyCode) -> Result<()> {
        let Some(position) = self.items_state.selected() else {
            return Ok(());
        };
        let next = match code {
            KeyCode::Left => self.tree.left(position),
            KeyCode::Right if self.selected_row().and_then(document_id).is_some() => {
                return self.focus(Pane::Body);
            }
            KeyCode::Right => self.tree.right(position),
            _ => {
                let page = usize::from((self.items_height / 2).max(1));
                Some(next_selection(
                    position,
                    self.tree.visible().count(),
                    code,
                    page,
                ))
            }
        };
        if let Some(next) = next {
            self.select_row(next)?;
        }
        Ok(())
    }

    fn show_linked_item(&mut self, item: Item) {
        if let Some(position) = self.tree.reveal(&item.id) {
            self.items_state.select(Some(position));
        }
        self.set_item(Some(item));
    }

    fn follow_item_link(&mut self, id: &str, from: Pane) -> Result<()> {
        let target = self.sara.item(id)?;
        self.remember_current(from);
        self.show_linked_item(target);
        self.pane = Pane::Body;
        self.body_tab = BodyTab::Body;
        Ok(())
    }

    fn remember_current(&mut self, from: Pane) {
        if let Some(item) = &self.item {
            self.history
                .push((item.id.clone(), from, self.body_scroll.offset));
        }
    }

    fn link_count(&self) -> usize {
        self.item
            .iter()
            .flat_map(|item| &item.links)
            .map(|relation| relation.items.len())
            .sum()
    }

    fn open_selected(&mut self) -> Result<()> {
        match self.pane {
            Pane::Items => {
                if let Some(position) = self.items_state.selected() {
                    match self.tree.row_at(position).map(|row| &row.kind) {
                        Some(RowKind::Document(_)) => self.focus(Pane::Body)?,
                        Some(RowKind::Group { .. }) => self.tree.toggle(position),
                        None => {}
                    }
                }
            }
            Pane::Links => {
                if let (Some(item), Some(index)) = (&self.item, self.links_state.selected()) {
                    let id = item
                        .links
                        .iter()
                        .flat_map(|relation| &relation.items)
                        .nth(index)
                        .map(|target| target.id.clone());
                    if let Some(id) = id {
                        self.follow_item_link(&id, Pane::Links)?;
                    }
                }
            }
            Pane::Body => {}
        }
        Ok(())
    }

    fn back(&mut self) -> Result<()> {
        if let Some((id, pane, offset)) = self.history.last().cloned() {
            let item = self.sara.item(&id)?;
            self.history.pop();
            self.show_linked_item(item);
            self.pane = pane;
            self.body_tab = BodyTab::Body;
            self.body_scroll.offset = offset;
        } else {
            self.focus(Pane::Items)?;
        }
        Ok(())
    }

    fn trace_is_visible(&self) -> bool {
        self.body_tab != BodyTab::Body && !self.body_area.is_empty()
    }

    pub fn handle_key(&mut self, mut key: KeyEvent) -> Action {
        let pending_g = std::mem::take(&mut self.pending_g);
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        if self.search.is_none() {
            if key.code == KeyCode::Char('q') {
                if self.overlay.is_none() && self.trace_is_visible() {
                    key.code = KeyCode::Esc;
                } else {
                    return Action::Quit;
                }
            }
            if key.code == KeyCode::Char('o')
                && key.modifiers.is_empty()
                && self.overlay.is_none()
                && self.pane == Pane::Body
                && self.body_tab == BodyTab::Body
                && self.item.is_some()
            {
                return Action::Edit;
            }
            if self.overlay.is_none() && matches!(self.pane, Pane::Items | Pane::Body) {
                match key.code {
                    KeyCode::Char('g') if key.modifiers.is_empty() => {
                        if !pending_g {
                            self.pending_g = true;
                            return Action::Continue;
                        }
                        key.code = KeyCode::Home;
                    }
                    KeyCode::Char('G')
                        if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                    {
                        key.code = KeyCode::End;
                    }
                    _ => {}
                }
            }
        }
        let result = self.update(key);
        self.report_error(result);
        Action::Continue
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        self.pending_g = false;
        let position = Position::new(event.column, event.row);
        if self.overlay.is_some() || self.search.is_some() || !self.body_area.contains(position) {
            return;
        }
        if let Some(traversal) = self.body_tab.traversal() {
            if let Some(Ok(trace)) = self.traces.get_mut(&traversal) {
                match event.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(&(_, index)) = trace
                            .visible_nodes
                            .iter()
                            .find(|(area, _)| area.contains(position))
                        {
                            let id = trace.diagram.nodes[index].id.clone();
                            let result = self.follow_item_link(&id, self.pane);
                            self.report_error(result);
                        }
                    }
                    MouseEventKind::ScrollUp => trace.handle_key(KeyCode::Up),
                    MouseEventKind::ScrollDown => trace.handle_key(KeyCode::Down),
                    _ => {}
                }
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(destination) = self
                    .body_links
                    .iter()
                    .find(|(area, _)| area.contains(position))
                    .and_then(|&(_, index)| self.markdown.destination(index))
                    .map(str::to_owned)
                {
                    let result = self.open_body_link(&destination);
                    self.report_error(result);
                }
            }
            MouseEventKind::ScrollUp => self.body_scroll.handle_key(KeyCode::Up),
            MouseEventKind::ScrollDown => self.body_scroll.handle_key(KeyCode::Down),
            _ => {}
        }
    }

    fn open_body_link(&mut self, destination: &str) -> Result<()> {
        if let Some(path) = destination.strip_prefix("//") {
            return open::that_detached(format!("https://{path}"))
                .context("リンクをブラウザで開けませんでした");
        }
        if let Some((scheme, _)) = destination.split_once(':') {
            if scheme.eq_ignore_ascii_case("http")
                || scheme.eq_ignore_ascii_case("https")
                || scheme.eq_ignore_ascii_case("mailto")
            {
                return open::that_detached(destination)
                    .context("リンクを既定のアプリで開けませんでした");
            }
        }
        let (href, fragment) = destination.split_once('#').unwrap_or((destination, ""));
        let href = href.split_once('?').map_or(href, |(path, _)| path);
        let anchor = percent_decode_str(fragment)
            .decode_utf8()
            .context("リンク先の見出しを読み取れません")?;
        if href.is_empty() && destination.contains('#') {
            self.remember_current(Pane::Body);
            self.pane = Pane::Body;
            self.body_anchor = Some(anchor.into_owned());
            return Ok(());
        }
        if !href.is_empty() && !href.split('/').next().unwrap_or_default().contains(':') {
            let item = self.item.as_ref().context("文書が表示されていません")?;
            let path = percent_decode_str(href)
                .decode_utf8()
                .context("リンク先のファイル名を読み取れません")?;
            let source = self.sara.project.join(&item.file);
            let target = source
                .parent()
                .context("文書のディレクトリを取得できません")?
                .join(path.as_ref())
                .canonicalize()
                .with_context(|| format!("リンク先が見つかりません（{destination}）"))?;
            let hit = self.sara.search("")?.into_iter().find(|hit| {
                self.sara
                    .project
                    .join(&hit.file)
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&target)
            });
            if let Some(hit) = hit {
                self.follow_item_link(&hit.item.id, Pane::Body)?;
                if !anchor.is_empty() {
                    self.body_anchor = Some(anchor.into_owned());
                }
                return Ok(());
            }
            if target.is_file()
                && target
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("md")
                            || extension.eq_ignore_ascii_case("markdown")
                    })
            {
                return open::that_detached(target)
                    .context("Markdown ファイルを既定のアプリで開けませんでした");
            }
        }
        bail!("リンク先を開けません（{destination}）");
    }

    pub fn report_error(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.show_overlay(Overlay::Error(format!("{error:#}")));
        }
    }

    pub fn edit_current(&mut self) -> Result<()> {
        let file = self
            .item
            .as_ref()
            .context("編集中の文書がありません")?
            .file
            .clone();
        let path = self.sara.project.join(&file);
        if !path.is_file() {
            bail!("Markdown ファイルを開けません（{}）", path.display());
        }
        let mut editor = editor_command()?;
        let program = editor.get_program().to_string_lossy().into_owned();
        let status = editor
            .arg(&path)
            .current_dir(&self.sara.project)
            .status()
            .with_context(|| format!("エディターを起動できません（{program}）"))?;
        let refreshed = self.refresh_edited_item(Path::new(&file));
        if !status.success() {
            bail!("エディターがエラーで終了しました（{program}、{status}）");
        }
        refreshed
    }

    fn refresh_edited_item(&mut self, file: &Path) -> Result<()> {
        let hits = self.sara.search(&self.query)?;
        let mut id = hits
            .iter()
            .find(|hit| hit.file == file)
            .map(|hit| hit.item.id.clone());
        if id.is_none() && !self.query.is_empty() {
            id = self
                .sara
                .search("")?
                .into_iter()
                .find(|hit| hit.file == file)
                .map(|hit| hit.item.id);
        }
        let id = id.with_context(|| {
            format!("編集した文書を Sara に読み込めません（{}）", file.display())
        })?;
        let item = self.sara.item(&id)?;
        let item_types = self.sara.item_types()?;
        let (mut tree, selected) = self.build_tree(hits, &item_types, &self.query, true);
        self.items_state.select(tree.reveal(&item.id).or(selected));
        self.tree = tree;
        let offset = self.body_scroll.offset;
        self.set_item(Some(item));
        self.body_scroll.offset = offset;
        Ok(())
    }

    fn update(&mut self, key: KeyEvent) -> Result<()> {
        let code = match key.code {
            KeyCode::Char('h') => KeyCode::Left,
            KeyCode::Char('j') => KeyCode::Down,
            KeyCode::Char('k') => KeyCode::Up,
            KeyCode::Char('l') => KeyCode::Right,
            KeyCode::Char('o') if self.pane == Pane::Links && key.modifiers.is_empty() => {
                KeyCode::Enter
            }
            code => code,
        };
        if let Some(overlay) = &self.overlay {
            let close = matches!(
                (code, &*overlay),
                (KeyCode::Esc, _) | (KeyCode::Char('?'), Overlay::Help)
            );
            if close {
                self.overlay = None;
            } else {
                self.overlay_scroll.handle_key(code);
            }
            return Ok(());
        }
        if let Some(search) = &mut self.search {
            match key.code {
                KeyCode::Esc => self.search = None,
                KeyCode::Enter => {
                    let query = search.trim().to_owned();
                    self.load_tree(query, false)?;
                    self.search = None;
                }
                KeyCode::Backspace => {
                    if let Some((index, _)) = search.grapheme_indices(true).next_back() {
                        search.truncate(index);
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    search.clear()
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    search.push(ch);
                }
                _ => {}
            }
            return Ok(());
        }

        match code {
            KeyCode::Char('?') => self.show_overlay(Overlay::Help),
            KeyCode::Char('/') => self.search = Some(String::new()),
            KeyCode::Esc if self.trace_is_visible() => self.body_tab = BodyTab::Body,
            KeyCode::Esc if !self.query.is_empty() => self.load_tree(String::new(), true)?,
            KeyCode::Char('1') | KeyCode::Esc => self.focus(Pane::Items)?,
            KeyCode::Left if self.pane == Pane::Body && self.body_tab == BodyTab::Body => {
                self.focus(Pane::Items)?
            }
            KeyCode::Char('2') => self.focus(Pane::Body)?,
            KeyCode::Char('3') => self.focus(Pane::Links)?,
            KeyCode::Char('[' | ']') if self.pane == Pane::Body && key.modifiers.is_empty() => {
                self.body_tab = self.body_tab.cycle(code == KeyCode::Char('['));
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let panes = [Pane::Items, Pane::Body, Pane::Links];
                let current = panes
                    .iter()
                    .position(|pane| *pane == self.pane)
                    .unwrap_or(0);
                let step = if code == KeyCode::BackTab { 2 } else { 1 };
                self.focus(panes[(current + step) % panes.len()])?;
            }
            KeyCode::Enter => self.open_selected()?,
            KeyCode::Char('o' | ' ') if self.pane == Pane::Items && key.modifiers.is_empty() => {
                self.open_selected()?
            }
            KeyCode::Char('b') => self.back()?,
            KeyCode::Char('r') => self.load_tree(self.query.clone(), true)?,
            KeyCode::Char('v') => {
                let validation = self.sara.validate()?;
                self.show_overlay(Overlay::Validation(validation));
            }
            _ => match self.pane {
                Pane::Items => self.move_in_tree(code)?,
                Pane::Links => {
                    if let Some(index) = self.links_state.selected() {
                        let next = next_selection(index, self.link_count(), code, 1);
                        self.links_state.select(Some(next));
                    }
                }
                Pane::Body => {
                    if let Some(traversal) = self.body_tab.traversal() {
                        if let Some(Ok(trace)) = self.traces.get_mut(&traversal) {
                            trace.handle_key(code);
                        }
                    } else {
                        self.body_scroll.handle_key(code);
                    }
                }
            },
        }
        Ok(())
    }

    pub fn trace_view(&mut self) -> Option<&mut std::result::Result<TraceView, String>> {
        let traversal = self.body_tab.traversal()?;
        let item = self.item.as_ref()?;
        Some(self.traces.entry(traversal).or_insert_with(|| {
            self.sara
                .trace_mermaid(&item.id, traversal)
                .and_then(|source| render_mermaid(&source))
                .map(|diagram| TraceView {
                    width: diagram.text.width(),
                    diagram,
                    left: None,
                    scroll: Scroll::default(),
                    visible_nodes: Vec::new(),
                })
                .map_err(|error| format!("{error:#}"))
        }))
    }

    pub fn paste(&mut self, text: &str) {
        self.pending_g = false;
        if self.overlay.is_none() {
            if let Some(search) = &mut self.search {
                search.push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
    }

    fn show_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
        self.overlay_scroll = Scroll::default();
    }
}

fn editor_command() -> Result<Command> {
    let setting = env::var_os("EDITOR").unwrap_or_default();
    let path = Path::new(&setting);
    if path.is_absolute() && path.is_file() {
        return Ok(Command::new(setting));
    }

    let setting = setting.to_str().context("$EDITOR を読み取れません")?;
    let mut args = shlex::split(setting)
        .context("$EDITOR のコマンドを読み取れません。引用符を確認してください")?
        .into_iter();
    let mut command = Command::new(args.next().as_deref().unwrap_or("vim"));
    command.args(args);
    Ok(command)
}

fn document_id(row: &Row) -> Option<&str> {
    match &row.kind {
        RowKind::Document(item) => Some(&item.id),
        RowKind::Group { .. } => None,
    }
}

fn next_selection(index: usize, count: usize, code: KeyCode, page: usize) -> usize {
    let last = count.saturating_sub(1);
    match code {
        KeyCode::Up => index.saturating_sub(1),
        KeyCode::Down => (index + 1).min(last),
        KeyCode::PageUp | KeyCode::Char('[') => index.saturating_sub(page),
        KeyCode::PageDown | KeyCode::Char(']') => (index + page).min(last),
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => index,
    }
}
