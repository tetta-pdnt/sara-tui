use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph, Wrap,
};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{App, Overlay, Pane, Scroll};
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
        Span::styled(" / ", Style::new().fg(ACCENT).bold()),
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
    let mut title = format!(" 1 文書  {} 件 ", app.tree.count);
    if !app.query.is_empty() {
        title.push_str(&format!(" 「{}」 ", app.query));
    }
    let block = panel(title, focused);
    let inner = block.inner(area);
    app.items_height = inner.height;
    let row_width = usize::from(inner.width).saturating_sub(Span::raw(LIST_PADDING).width());
    if app.tree.count == 0 {
        frame.render_widget(
            Paragraph::new("該当する文書がありません。\n/ で検索条件を変更できます。").block(block),
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
    let Some(item) = &app.item else {
        frame.render_widget(
            Paragraph::new("文書を選ぶと本文を表示します。").block(panel(" 2 本文 ", focused)),
            area,
        );
        return;
    };
    let title = format!(" 2 本文  {}  {} ", item.id, item.type_name);
    let block = panel(title, focused);
    let inner = block.inner(area);
    let markdown = item.body.as_deref().filter(|body| !body.trim().is_empty());
    let preview = markdown.map(|body| app.markdown.at_width(body, inner.width));
    let text = item_text(item, preview);
    frame.render_widget(block, area);
    draw_scroll(frame, inner, text, &mut app.body_scroll);
}

fn item_text<'a>(item: &'a Item, preview: Option<&Text<'static>>) -> Text<'a> {
    let mut lines = vec![
        Line::styled(item.name.as_str(), Style::new().bold()),
        Line::styled(item.file.as_str(), Style::new().fg(MUTED)),
        Line::default(),
    ];
    if let Some(description) = &item.description {
        lines.extend(description.lines().map(Line::from));
        lines.push(Line::default());
    }
    match preview {
        Some(body) => lines.extend(body.lines.iter().cloned()),
        None => lines.push(Line::styled(
            "Markdown 本文は空です。",
            Style::new().fg(MUTED),
        )),
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
    Text::from(lines)
}

fn draw_links(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.pane == Pane::Links && app.search.is_none();
    let block = panel(" 3 関連文書 ", focused);
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
        frame.render_widget(Paragraph::new("関連文書はありません。").block(block), area);
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
        Overlay::Help => (" キーバインド ", help_text()),
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

fn help_text() -> Text<'static> {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "共通",
            &[
                ("1 / 2 / 3", "文書ツリー / 本文 / 関連文書に移る"),
                ("Tab / Shift+Tab", "前後の枠に切り替える"),
                ("↑ ↓ / k j", "選択またはスクロールを上下する"),
                ("Home / End", "先頭 / 末尾に移る"),
                ("?", "キーバインド一覧を開く・閉じる"),
                ("Esc", "絞り込みを解除して文書ツリーに戻る"),
                ("b", "関連文書をたどる前の文書に戻る"),
                ("r / v", "文書を読み直す / モデルを検証する"),
                ("q / Ctrl+C", "終了する"),
            ],
        ),
        (
            "検索",
            &[
                ("/", "検索欄を開く"),
                ("Enter / Esc", "検索を確定 / 取り消す"),
                ("Backspace / Ctrl+U", "末尾の文字 / 入力全体を消す"),
            ],
        ),
        (
            "1 文書ツリー",
            &[
                (
                    "Enter / o / Space",
                    "グループを開閉するか、文書から本文へ移る",
                ),
                ("← / h", "グループを閉じるか、親グループに移る"),
                ("→ / l", "グループを開くか、先頭の文書に移る"),
                ("→ / l", "選択中の文書から本文へ移る"),
                ("PageUp / PageDown / [ ]", "選択を画面単位で移す"),
            ],
        ),
        (
            "2 本文",
            &[
                ("← / h", "文書ツリーへ移る"),
                ("o", "元の Markdown を $EDITOR で編集する"),
                ("PageUp / PageDown", "本文を画面単位でスクロールする"),
            ],
        ),
        ("3 関連文書", &[("Enter", "選択したリンクをたどる")]),
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
    let limit = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height));
    scroll.height = inner.height;
    scroll.limit = u16::try_from(limit).unwrap_or(u16::MAX);
    scroll.offset = scroll.offset.min(scroll.limit);
    frame.render_widget(paragraph.scroll((scroll.offset, 0)), inner);
}
