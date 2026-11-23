use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, Padding, Paragraph, Wrap,
};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{App, BodyTab, Overlay, Pane, Scroll};
use crate::markdown::{MermaidDiagram, RenderedMarkdown};
use crate::sara::{Direction, Field, Item};
use crate::tree::RowKind;

const BACKGROUND: Color = Color::Rgb(16, 21, 39);
const SURFACE: Color = Color::Rgb(23, 28, 46);
const FOREGROUND: Color = Color::Rgb(233, 233, 243);
const MUTED: Color = Color::Rgb(164, 174, 202);
const ACCENT: Color = Color::Rgb(202, 191, 255);
const SELECTED_BACKGROUND: Color = Color::Rgb(52, 56, 102);
const SELECTION: Style = Style::new().fg(FOREGROUND).bg(SELECTED_BACKGROUND);
const LIST_PADDING: &str = "  ";

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.body_area = Rect::ZERO;
    app.body_links.clear();
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::new().fg(FOREGROUND).bg(BACKGROUND)),
        area,
    );
    if area.width < 32 || area.height < 10 {
        frame.render_widget(Paragraph::new("Sara TUI\n端末を広げてください。"), area);
        return;
    }

    let search_input = app.search.as_deref().filter(|_| app.overlay.is_none());
    let [header, main, search] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(search_input.is_some())),
    ])
    .areas(area);

    let project = app
        .sara
        .project
        .file_name()
        .unwrap_or(app.sara.project.as_os_str());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Sara ", Style::new().bg(ACCENT).fg(BACKGROUND).bold()),
            Span::styled(
                format!("  {}", project.to_string_lossy()),
                Style::new().fg(MUTED),
            ),
        ])),
        header,
    );
    if let Some(value) = search_input {
        draw_search(frame, value, search);
    }
    if matches!(app.overlay, None | Some(Overlay::Help)) {
        draw_workspace(frame, main, app);
    }
    if let Some(overlay) = &app.overlay {
        let window = if matches!(overlay, Overlay::Help) {
            let window = help_window(main);
            // A wide character at the window edge can occupy the next column.
            let backdrop = Rect::new(window.x - 1, window.y, window.width + 2, window.height);
            frame.render_widget(Clear, backdrop);
            frame.render_widget(Block::default().bg(BACKGROUND), backdrop);
            window
        } else {
            main
        };
        draw_overlay(frame, window, overlay, &mut app.overlay_scroll);
    }
}

fn draw_workspace(frame: &mut Frame, main: Rect, app: &mut App) {
    if main.width >= 64 {
        let sidebar_width = (main.width / 5 * 2).clamp(24, 38);
        let [items, detail] =
            Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)]).areas(main);
        draw_items(frame, items, app);
        if main.height >= 12 {
            let links_height = if app.pane == Pane::Links {
                main.height / 2
            } else {
                (main.height / 3).min(7)
            };
            let [body, links] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(links_height)])
                    .areas(detail);
            draw_body(frame, body, app);
            draw_links(frame, links, app);
        } else if app.pane == Pane::Links {
            draw_links(frame, detail, app);
        } else {
            draw_body(frame, detail, app);
        }
    } else {
        match app.pane {
            Pane::Items if main.height >= 10 => {
                let [items, body] =
                    Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .areas(main);
                draw_items(frame, items, app);
                draw_body(frame, body, app);
            }
            Pane::Items => draw_items(frame, main, app),
            Pane::Body => draw_body(frame, main, app),
            Pane::Links => draw_links(frame, main, app),
        }
    }
}

fn help_window(area: Rect) -> Rect {
    let width = (area.width - area.width / 5).clamp(28, 76);
    let height = area.height.saturating_sub(2).min(32);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_search(frame: &mut Frame, value: &str, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" Search ", Style::new().fg(ACCENT).bold()),
        Span::raw(value),
    ]);
    let width = u16::try_from(line.width()).unwrap_or(u16::MAX);
    let offset = width.saturating_sub(area.width - 1);
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::new().bg(SURFACE))
            .scroll((0, offset)),
        area,
    );
    frame.set_cursor_position(Position::new(area.x + width.saturating_sub(offset), area.y));
}

fn panel(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(if focused { ACCENT } else { SELECTED_BACKGROUND }))
        .style(Style::new().fg(FOREGROUND).bg(SURFACE))
}

fn navigation_list<'a>(items: Vec<ListItem<'a>>, block: Block<'static>) -> List<'a> {
    List::new(items)
        .block(block)
        .highlight_symbol(LIST_PADDING)
        .highlight_spacing(HighlightSpacing::Always)
}

fn draw_items(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.pane == Pane::Items && app.search.is_none();
    let mut title = format!(" [1] Items  {} ", app.tree.count);
    if !app.query.is_empty() {
        title.push_str(&format!(" 「{}」 ", app.query));
    }
    let block = panel(title, focused).padding(Padding::right(2));
    let inner = block.inner(area);
    app.items_height = inner.height;
    let row_width = usize::from(inner.width).saturating_sub(Span::raw(LIST_PADDING).width());
    if app.tree.count == 0 {
        frame.render_widget(
            Paragraph::new("No matching items.\nPress / to search.").block(block),
            area,
        );
        return;
    }
    let items = app
        .tree
        .visible()
        .map(|(_, row)| match &row.kind {
            RowKind::Group {
                type_id,
                name,
                count,
            } => {
                let arrow = if app.tree.expanded(type_id) {
                    '▾'
                } else {
                    '▸'
                };
                ListItem::new(group_line(format!("{arrow} {name}"), *count, row_width))
            }
            RowKind::Document(item) => ListItem::new(Text::from(vec![
                Line::styled(format!("  {}", item.id), Style::new().fg(ACCENT).bold()),
                Line::from(format!("  {}", item.name)),
            ])),
        })
        .collect();
    let mut list = navigation_list(items, block);
    if app.sidebar_matches_preview() {
        list = list.highlight_style(SELECTION);
    }
    frame.render_stateful_widget(list, area, &mut app.items_state);
}

fn group_line(mut label: String, count: usize, width: usize) -> Line<'static> {
    let count = count.to_string();
    let label_limit = width.saturating_sub(count.len() + 3);
    if Span::raw(label.as_str()).width() > label_limit {
        let mut remaining = label_limit.saturating_sub(1);
        let mut end = 0;
        for (index, grapheme) in label.grapheme_indices(true) {
            let grapheme_width = Span::raw(grapheme).width();
            if grapheme_width > remaining {
                break;
            }
            remaining -= grapheme_width;
            end = index + grapheme.len();
        }
        label.truncate(end);
        if label_limit > 0 {
            label.push('…');
        }
    }
    let spacing = width.saturating_sub(Span::raw(label.as_str()).width() + count.len());
    let divider = if spacing >= 3 {
        format!(" {} ", "─".repeat(spacing - 2))
    } else {
        " ".repeat(spacing)
    };
    Line::from(vec![
        Span::styled(label, Style::new().fg(MUTED).bold()),
        Span::styled(divider, Style::new().fg(SELECTED_BACKGROUND)),
        Span::styled(count, Style::new().fg(MUTED)),
    ])
}

fn draw_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.pane == Pane::Body && app.search.is_none();
    let mut title = Line::from(" [2] ");
    title.spans.extend(BodyTab::ALL.map(|tab| {
        let style = match (focused, tab == app.body_tab) {
            (true, true) => SELECTION.bold(),
            (true, false) => Style::new().fg(MUTED),
            (false, true) => Style::new().fg(MUTED).bg(Color::Rgb(35, 40, 70)).bold(),
            (false, false) => Style::new().fg(Color::Rgb(106, 117, 148)),
        };
        Span::styled(format!("{} ", tab.label()), style)
    }));
    if let Some(item) = &app.item {
        let width = usize::from(area.width.saturating_sub(2));
        for label in [format!("{} ", item.id), format!(" {} ", item.type_name)] {
            let label = Span::raw(label);
            if title.width() + label.width() > width {
                break;
            }
            title.spans.push(label);
        }
    }
    let block = panel(title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let inner = if app.body_tab == BodyTab::Body {
        inner.inner(Margin::new(1, 1))
    } else {
        inner
    };
    app.body_area = inner;
    if inner.is_empty() {
        return;
    }
    if let Some(trace) = app.trace_view() {
        match trace {
            Ok(trace) => {
                let limit = trace.width.saturating_sub(usize::from(inner.width));
                let left = trace.left.get_or_insert(limit / 2);
                *left = (*left).min(limit);
                trace
                    .scroll
                    .set_viewport(trace.diagram.text.height(), inner.height);
                trace.visible_nodes.clear();
                draw_diagram(
                    frame,
                    inner,
                    &trace.diagram,
                    *left,
                    trace.scroll.offset,
                    &mut trace.visible_nodes,
                );
            }
            Err(error) => frame.render_widget(
                Paragraph::new(error.as_str())
                    .style(Style::new().fg(Color::LightRed))
                    .wrap(Wrap { trim: false }),
                inner,
            ),
        }
        return;
    }

    let Some(item) = &app.item else {
        frame.render_widget(Paragraph::new("Select an item to view its body."), inner);
        return;
    };
    let markdown = item.body.as_deref().unwrap_or_default();
    let preview = app.markdown.at_width(&item.name, markdown, inner.width);
    let (text, markdown_start) = item_text(item, preview, inner.width);
    let missing_anchor = app.body_anchor.take().and_then(|anchor| {
        if anchor.is_empty() || preview.title_anchor.as_deref() == Some(&anchor) {
            app.body_scroll.offset = 0;
        } else if let Some(line) = preview.headings.get(&anchor) {
            let preceding = Text::from(text.lines[..markdown_start + line].to_vec());
            let offset = Paragraph::new(preceding)
                .wrap(Wrap { trim: false })
                .line_count(inner.width);
            app.body_scroll.offset = u16::try_from(offset).unwrap_or(u16::MAX);
        } else {
            return Some(anchor);
        }
        None
    });
    draw_scroll(frame, inner, text, &mut app.body_scroll);
    if preview.has_links() {
        collect_body_links(frame, inner, &mut app.body_links);
    }
    if let Some(anchor) = missing_anchor {
        app.report_error(Err(anyhow::anyhow!(
            "リンク先の見出しが見つかりません（{anchor}）"
        )));
    }
}

fn collect_body_links(frame: &mut Frame, area: Rect, links: &mut Vec<(Rect, usize)>) {
    let buffer = frame.buffer_mut();
    for position in area.positions() {
        let cell = &mut buffer[position];
        let Color::Rgb(r, g, b) = cell.underline_color else {
            continue;
        };
        let index = (usize::from(r) << 16) | (usize::from(g) << 8) | usize::from(b);
        cell.underline_color = Color::Reset;
        let width = (Span::raw(cell.symbol()).width() as u16).min(area.right() - position.x);
        if let Some((last, last_index)) = links.last_mut() {
            if *last_index == index && last.y == position.y && last.right() == position.x {
                last.width += width;
                continue;
            }
        }
        links.push((Rect::new(position.x, position.y, width, 1), index));
    }
}

fn item_text<'a>(item: &'a Item, preview: &RenderedMarkdown, width: u16) -> (Text<'a>, usize) {
    let mut lines = preview.title.clone();
    lines.extend([
        Line::styled(item.file.as_str(), Style::new().fg(MUTED)),
        Line::default(),
    ]);
    if let Some(description) = &item.description {
        lines.extend(description.lines().map(Line::from));
        lines.push(Line::default());
    }
    let markdown_start = lines.len();
    match &preview.body {
        Some(body) => lines.extend(body.lines.iter().cloned()),
        None => lines.push(Line::styled("The body is empty.", Style::new().fg(MUTED))),
    }
    if !item.fields.is_empty() {
        let label = "Fields ";
        let rule_width = usize::from(width).saturating_sub(label.len());
        lines.extend([Line::default(), Line::default()]);
        lines.push(Line::from(vec![
            Span::styled(label, Style::new().fg(MUTED).bold()),
            Span::styled("─".repeat(rule_width), Style::new().fg(SELECTED_BACKGROUND)),
        ]));
    }
    for (label, value) in &item.fields {
        lines.push(Line::default());
        lines.push(Line::styled(label.as_str(), Style::new().fg(ACCENT)));
        match value {
            Field::Text(value) => lines.extend(value.lines().map(Line::from)),
            Field::List(values) => {
                lines.extend(values.iter().map(|value| Line::from(format!("- {value}"))))
            }
        }
    }
    (Text::from(lines), markdown_start)
}

fn draw_links(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.pane == Pane::Links && app.search.is_none();
    let block = panel(" [3] Related Items ", focused);
    let items: Vec<_> = app
        .item
        .iter()
        .flat_map(|item| &item.links)
        .flat_map(|relation| {
            let direction = match relation.direction {
                Some(Direction::Upstream) => "↑",
                Some(Direction::Downstream) => "↓",
                Some(Direction::Peer) => "↔",
                None => "·",
            };
            relation.items.iter().map(move |item| {
                ListItem::new(Text::from(vec![
                    Line::from(vec![
                        Span::styled(
                            format!("{direction} {}", relation.label),
                            Style::new().fg(MUTED),
                        ),
                        Span::styled(format!("  {}", item.id), Style::new().fg(ACCENT).bold()),
                    ]),
                    Line::from(item.name.as_str()),
                ]))
            })
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(Paragraph::new("No related items.").block(block), area);
    } else {
        let mut list = navigation_list(items, block);
        if focused {
            list = list.highlight_style(SELECTION);
        }
        frame.render_stateful_widget(list, area, &mut app.links_state);
    }
}

fn draw_overlay(frame: &mut Frame, area: Rect, overlay: &Overlay, scroll: &mut Scroll) {
    let (title, text) = match overlay {
        Overlay::Help => (" Key Bindings ", help_text()),
        Overlay::Error(message) => (" エラー ", Text::from(message.as_str())),
        Overlay::Validation(report) => {
            let status = if report.valid {
                "検証に合格"
            } else {
                "検証に失敗"
            };
            let mut lines = vec![
                Line::styled(
                    status,
                    Style::new()
                        .fg(if report.valid {
                            ACCENT
                        } else {
                            Color::LightRed
                        })
                        .bold(),
                ),
                Line::from(format!(
                    "{} 文書 / {} リンク / エラー {} 件 / 警告 {} 件",
                    report.items_checked,
                    report.relationships_checked,
                    report.error_count,
                    report.warning_count
                )),
                Line::default(),
            ];
            for issue in &report.issues {
                let (label, color) = if issue.severity == "error" {
                    ("エラー", Color::LightRed)
                } else {
                    ("警告", Color::LightYellow)
                };
                lines.push(Line::styled(label, Style::new().fg(color)));
                lines.extend(issue.message.lines().map(Line::from));
                lines.push(Line::default());
            }
            for warning in &report.scan_warnings {
                lines.push(Line::styled(
                    "読み込みをスキップ",
                    Style::new().fg(Color::LightYellow),
                ));
                lines.push(Line::from(warning.path.as_str()));
                lines.extend(warning.reason.lines().map(Line::from));
                lines.push(Line::default());
            }
            (" モデル検証 ", Text::from(lines))
        }
    };
    let block = panel(title, true);
    let mut inner = block.inner(area);
    inner.width = inner.width.min(90);
    frame.render_widget(block, area);
    draw_scroll(frame, inner, text, scroll);
}

fn draw_diagram(
    frame: &mut Frame,
    area: Rect,
    diagram: &MermaidDiagram,
    left: usize,
    top: u16,
    visible_nodes: &mut Vec<(Rect, usize)>,
) {
    let top = usize::from(top);
    let right = left + usize::from(area.width);
    let buffer = frame.buffer_mut();
    let lines = diagram.text.lines.iter().skip(top);
    for (y, line) in (area.top()..area.bottom()).zip(lines) {
        let mut column = 0;
        for grapheme in line.styled_graphemes(diagram.text.style) {
            let width = Span::raw(grapheme.symbol).width();
            let end = column + width;
            // A wide glyph straddling an edge must leave its visible cells blank.
            if end > right {
                break;
            }
            if column >= left && width > 0 {
                let x = area.left() + (column - left) as u16;
                buffer[(x, y)]
                    .set_symbol(grapheme.symbol)
                    .set_style(grapheme.style);
            }
            column = end;
        }
    }
    let visible_rows = top..top + usize::from(area.height);
    let link_style = Style::new().fg(ACCENT).underlined();
    for (index, node) in diagram.nodes.iter().enumerate() {
        if !visible_rows.contains(&node.row) {
            continue;
        }
        let start = node.columns.start.max(left);
        let end = node.columns.end.min(right);
        let y = area.top() + (node.row - top) as u16;
        let mut hitbox = Rect::new(0, y, 0, 1);
        for column in start..end {
            let x = area.left() + (column - left) as u16;
            let cell = &mut buffer[(x, y)];
            // Blank cells include partially clipped wide glyphs.
            if cell.symbol() == " " {
                continue;
            }
            let width = Span::raw(cell.symbol()).width() as u16;
            if hitbox.width == 0 {
                hitbox.x = x;
            }
            hitbox.width = x + width - hitbox.x;
            cell.set_style(link_style);
        }
        if hitbox.width > 0 {
            visible_nodes.push((hitbox, index));
        }
    }
}

fn help_text() -> Text<'static> {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "共通",
            &[
                ("1 / 2 / 3", "Items / Body / Related Items に移る"),
                ("Tab / Shift+Tab", "前後の枠に切り替える"),
                ("↑ ↓ / k j", "選択またはスクロールを上下する"),
                ("Home / End", "先頭 / 末尾に移る"),
                ("gg / G", "Items / Body で先頭 / 末尾に移る"),
                ("?", "Key Bindings を開く・閉じる"),
                ("Esc", "絞り込みを解除して Items に戻る"),
                ("b", "リンクをたどる前の Item に戻る"),
                ("r / v", "Item を読み直す / モデルを検証する"),
                ("q / Ctrl+C", "終了する（関係図では q で Body タブへ）"),
            ],
        ),
        (
            "Search",
            &[
                ("/", "検索欄を開く"),
                ("Enter / Esc", "検索を確定 / 取り消す"),
                ("Backspace / Ctrl+U", "末尾の文字 / 入力全体を消す"),
            ],
        ),
        (
            "[1] Items",
            &[
                ("Enter / o / Space", "グループを開閉するか、Body に移る"),
                ("← / h", "グループを閉じるか、親グループに移る"),
                ("→ / l", "グループを開くか、先頭の Item に移る"),
                ("→ / l", "選択中の Item から Body に移る"),
                ("PageUp / PageDown / [ ]", "選択を画面単位で移す"),
            ],
        ),
        (
            "[2] Body",
            &[
                ("[ / ]", "Body / Upstream / Downstream タブを切り替える"),
                ("← / h", "Body タブから Items に移る"),
                ("矢印 / hjkl", "Upstream・Downstream の図を移動する"),
                ("左クリック", "図の ID や本文中のリンクを開く"),
                ("o", "元の Markdown を $EDITOR で編集する"),
                ("PageUp / PageDown", "本文を画面単位でスクロールする"),
                ("Esc / q", "関係図から Body タブに戻る"),
            ],
        ),
        (
            "[3] Related Items",
            &[("Enter / o", "選択したリンクをたどる")],
        ),
    ];

    let mut lines = Vec::new();
    for (title, bindings) in sections {
        lines.push(Line::styled(*title, Style::new().bold()));
        lines.extend(bindings.iter().map(|&(key, label)| {
            Line::from(vec![
                Span::styled(key, Style::new().fg(ACCENT).bold()),
                Span::styled(format!(" {label}"), Style::new().fg(MUTED)),
            ])
        }));
        lines.push(Line::default());
    }
    Text::from(lines)
}

fn draw_scroll(frame: &mut Frame, inner: Rect, text: Text<'_>, scroll: &mut Scroll) {
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    scroll.set_viewport(paragraph.line_count(inner.width), inner.height);
    frame.render_widget(paragraph.scroll((scroll.offset, 0)), inner);
}
