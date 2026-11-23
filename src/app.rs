use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use unicode_segmentation::UnicodeSegmentation;

use crate::markdown::MarkdownPreview;
use crate::sara::{Item, ItemType, Sara, SearchHit, Validation};
use crate::tree::{Row, RowKind, Tree};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Items,
    Body,
    Links,
}

pub enum Overlay {
    Help,
    Validation(Validation),
    Error(String),
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
    pub markdown: MarkdownPreview,
    pub body_scroll: Scroll,
    pub overlay: Option<Overlay>,
    pub overlay_scroll: Scroll,
    history: Vec<String>,
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
            markdown: MarkdownPreview::default(),
            body_scroll: Scroll::default(),
            overlay: None,
            overlay_scroll: Scroll::default(),
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
        self.links_state = ListState::default().with_selected((self.link_count() > 0).then_some(0));
        self.body_scroll = Scroll::default();
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
                    if let Some(target) = item
                        .links
                        .iter()
                        .flat_map(|relation| &relation.items)
                        .nth(index)
                    {
                        let target = self.sara.item(&target.id)?;
                        self.history.push(item.id.clone());
                        self.show_linked_item(target);
                        self.pane = Pane::Body;
                    }
                }
            }
            Pane::Body => {}
        }
        Ok(())
    }

    fn back(&mut self) -> Result<()> {
        if let Some(id) = self.history.last() {
            let item = self.sara.item(id)?;
            self.history.pop();
            self.show_linked_item(item);
            self.pane = Pane::Links;
        } else {
            self.focus(Pane::Items)?;
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        if self.search.is_none() {
            if key.code == KeyCode::Char('q') {
                return Action::Quit;
            }
            if key.code == KeyCode::Char('o')
                && key.modifiers.is_empty()
                && self.overlay.is_none()
                && self.pane == Pane::Body
                && self.item.is_some()
            {
                return Action::Edit;
            }
        }
        let result = self.update(key);
        self.report_error(result);
        Action::Continue
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
            code => code,
        };
        if let Some(overlay) = &self.overlay {
            if code == KeyCode::Esc
                || (code == KeyCode::Char('?') && matches!(overlay, Overlay::Help))
            {
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
            KeyCode::Esc if !self.query.is_empty() => self.load_tree(String::new(), true)?,
            KeyCode::Char('1') | KeyCode::Esc => self.focus(Pane::Items)?,
            KeyCode::Left if self.pane == Pane::Body => self.focus(Pane::Items)?,
            KeyCode::Char('2') => self.focus(Pane::Body)?,
            KeyCode::Char('3') => self.focus(Pane::Links)?,
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
                Pane::Body => self.body_scroll.handle_key(code),
            },
        }
        Ok(())
    }

    pub fn paste(&mut self, text: &str) {
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
