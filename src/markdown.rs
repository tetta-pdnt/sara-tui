use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::ops::Range;
use std::process::{Command, Stdio};

use ansi_to_tui::IntoText;
use anyhow::{bail, Context, Result};
use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};

// Leaf's inline output keeps the styled link label but drops its destination.
// Zero-width markers carry an index through Leaf's layout without taking up a column.
const MARKER: &str = "\u{2063}\u{2063}";
const TAG_OFFSET: u32 = 0xe0000;

#[derive(Default)]
pub struct MarkdownPreview {
    rendered: Option<(u16, RenderedMarkdown)>,
}

#[derive(Default)]
pub struct RenderedMarkdown {
    pub title: Vec<Line<'static>>,
    pub title_anchor: Option<String>,
    pub body: Option<Text<'static>>,
    destinations: Vec<String>,
    pub headings: HashMap<String, usize>,
}

impl MarkdownPreview {
    pub fn at_width(&mut self, name: &str, markdown: &str, width: u16) -> &RenderedMarkdown {
        let width = width.max(20);
        let (rendered_width, text) = self
            .rendered
            .get_or_insert_with(|| (width, render(name, markdown, width)));
        if *rendered_width != width {
            *text = render(name, markdown, width);
            *rendered_width = width;
        }
        text
    }

    pub fn destination(&self, index: usize) -> Option<&str> {
        self.rendered
            .as_ref()?
            .1
            .destinations
            .get(index)
            .map(String::as_str)
    }
}

impl RenderedMarkdown {
    pub fn has_links(&self) -> bool {
        !self.destinations.is_empty()
    }
}

fn render(name: &str, markdown: &str, width: u16) -> RenderedMarkdown {
    let title = render_title(name, width);
    if markdown.trim().is_empty() {
        return RenderedMarkdown {
            title,
            ..Default::default()
        };
    }
    let MarkedMarkdown {
        source,
        destinations,
        headings,
        title_anchor,
    } = mark_markdown(name, markdown);
    let body = if source.trim().is_empty() {
        Ok(Text::default())
    } else {
        render_with_leaf(&source, width)
    };
    match body {
        Ok(mut text) => {
            let headings = take_markers(&mut text, destinations.len(), &headings);
            RenderedMarkdown {
                title,
                title_anchor,
                body: Some(text),
                destinations,
                headings,
            }
        }
        Err(error) => {
            let mut text = Text::from(vec![
                Line::styled(
                    "leaf で本文を表示できません。",
                    Style::new().fg(Color::LightRed),
                ),
                Line::default(),
            ]);
            text.lines.extend(Text::from(format!("{error:#}")).lines);
            RenderedMarkdown {
                title,
                title_anchor,
                body: Some(text),
                ..Default::default()
            }
        }
    }
}

fn render_title(name: &str, width: u16) -> Vec<Line<'static>> {
    let mut source = String::from("# ");
    for ch in name.chars() {
        if ch.is_ascii_punctuation() {
            source.push('\\');
        }
        source.push(match ch {
            '\r' | '\n' => ' ',
            ch => ch,
        });
    }
    match render_with_leaf(&source, width) {
        Ok(title) if !title.lines.is_empty() => title.lines,
        _ => vec![Line::styled(name.to_owned(), Style::new().bold())],
    }
}

struct MarkedMarkdown {
    source: String,
    destinations: Vec<String>,
    headings: Vec<String>,
    title_anchor: Option<String>,
}

fn mark_markdown(name: &str, markdown: &str) -> MarkedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_MATH;
    let mut destinations = Vec::new();
    let mut headings = Vec::new();
    let mut marks = Vec::new();
    let mut heading: Option<(String, Option<usize>, Option<CowStr<'_>>)> = None;
    let mut slug_counts = HashMap::new();
    let mut leading_h1_end = None;
    let mut body_start = 0;
    let mut title_anchor = None;
    for (index, (event, range)) in Parser::new_ext(markdown, options)
        .into_offset_iter()
        .enumerate()
    {
        if let Some((_, first, _)) = &mut heading {
            if !matches!(event, Event::End(TagEnd::Heading(_))) {
                first.get_or_insert(range.start);
            }
        }
        match event {
            Event::Start(Tag::Heading { id, level, .. }) => {
                heading = Some((String::new(), None, id));
                if index == 0 && level == HeadingLevel::H1 {
                    leading_h1_end = Some(range.end);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((label, _, _)) = &mut heading {
                    label.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                let end = leading_h1_end.take();
                if let Some((label, Some(start), id)) = heading.take() {
                    let anchor = if let Some(id) = id {
                        id.into_string()
                    } else {
                        let mut slug: String = label
                            .trim()
                            .chars()
                            .flat_map(char::to_lowercase)
                            .filter_map(|ch| match ch {
                                ch if ch.is_whitespace() => Some('-'),
                                '-' | '_' => Some(ch),
                                ch if ch.is_alphanumeric() => Some(ch),
                                _ => None,
                            })
                            .collect();
                        let count = slug_counts.entry(slug.clone()).or_insert(0);
                        if *count > 0 {
                            slug.push_str(&format!("-{count}"));
                        }
                        *count += 1;
                        slug
                    };
                    if let Some(end) = end.filter(|_| label.trim() == name.trim()) {
                        body_start = end;
                        title_anchor = Some(anchor);
                    } else {
                        marks.push((start, marker(&format!("h{}", headings.len()))));
                        headings.push(anchor);
                    }
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                // The name is plain text, so keep a heading with a clickable label.
                leading_h1_end = None;
                marks.push((range.start, marker(&format!("l{}", destinations.len()))));
                marks.push((range.end, marker("e")));
                destinations.push(dest_url.into_string());
            }
            Event::Start(Tag::Image { .. })
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_) => leading_h1_end = None,
            _ => {}
        }
    }

    // When two links are adjacent, close the first before starting the second.
    marks.sort_by_key(|(offset, _)| *offset);
    let mut source = String::with_capacity(markdown.len() - body_start);
    if body_start > 0 {
        source.push('\n');
    }
    let mut previous = body_start;
    for (offset, mark) in marks {
        source.push_str(&markdown[previous..offset]);
        source.push_str(&mark);
        previous = offset;
    }
    source.push_str(&markdown[previous..]);
    MarkedMarkdown {
        source,
        destinations,
        headings,
        title_anchor,
    }
}

fn marker(tag: &str) -> String {
    let mut result = MARKER.to_owned();
    result.extend(
        tag.bytes()
            .map(|byte| char::from_u32(TAG_OFFSET + u32::from(byte)).unwrap()),
    );
    result.push_str(MARKER);
    result
}

fn take_markers(
    text: &mut Text<'static>,
    link_count: usize,
    headings: &[String],
) -> HashMap<String, usize> {
    let mut heading_lines = HashMap::new();
    if link_count == 0 && headings.is_empty() {
        return heading_lines;
    }
    let mut pending = None;
    let mut positions = Vec::new();
    let mut links = Vec::new();
    for (row, line) in text.lines.iter_mut().enumerate() {
        for span in std::mem::take(&mut line.spans) {
            let mut content = span.content.as_ref();
            loop {
                let tag = content.split_once(MARKER).and_then(|(before, after)| {
                    let (tag, after) = after.split_once(MARKER)?;
                    Some((before, tag, after))
                });
                let before = tag.map_or(content, |(before, _, _)| before);
                if !before.is_empty() {
                    if pending.is_some() {
                        positions.push((row, line.spans.len()));
                    }
                    line.spans.push(Span::styled(before.to_owned(), span.style));
                }
                let Some((_, tag, rest)) = tag else {
                    break;
                };
                content = rest;
                if tag == "\u{e0065}" {
                    if let Some(index) = pending.take() {
                        links.push((index, std::mem::take(&mut positions)));
                    }
                } else if let Some(digits) = tag.strip_prefix('\u{e006c}') {
                    positions.clear();
                    pending = marker_index(digits).filter(|&index| index < link_count);
                } else if let Some(digits) = tag.strip_prefix('\u{e0068}') {
                    if let Some(name) = marker_index(digits).and_then(|index| headings.get(index)) {
                        heading_lines.entry(name.clone()).or_insert(row);
                    }
                }
            }
        }
    }
    for (index, positions) in links {
        // The frame reads this temporary style channel after wrapping, then clears it.
        if index > 0xffffff {
            continue;
        }
        let color = Color::Rgb((index >> 16) as u8, (index >> 8) as u8, index as u8);
        for (row, column) in positions {
            text.lines[row].spans[column].style.underline_color = Some(color);
        }
    }
    heading_lines
}

fn marker_index(digits: &str) -> Option<usize> {
    digits.chars().try_fold(0usize, |index, digit| {
        let digit = (digit as u32)
            .checked_sub(TAG_OFFSET + u32::from(b'0'))
            .filter(|&digit| digit < 10)?;
        index.checked_mul(10)?.checked_add(digit as usize)
    })
}

pub struct MermaidDiagram {
    pub text: Text<'static>,
    pub nodes: Vec<MermaidNode>,
}

pub struct MermaidNode {
    pub id: String,
    pub row: usize,
    pub columns: Range<usize>,
}

pub fn render_mermaid(source: &str) -> Result<MermaidDiagram> {
    // A narrow Leaf viewport falls back to source code. Render the complete diagram
    // so the TUI can show a scrollable portion without reflowing the connectors.
    let markdown = format!("```mermaid\n{}\n```\n", source.trim_end());
    let text =
        render_with_leaf(&markdown, u16::MAX).context("Mermaid の関係図を描画できませんでした")?;
    let nodes = mermaid_nodes(source, &text);
    Ok(MermaidDiagram { text, nodes })
}

fn mermaid_nodes(source: &str, text: &Text<'_>) -> Vec<MermaidNode> {
    let ids: HashSet<_> = source
        .lines()
        .filter_map(|line| {
            let (_, label) = line.trim().strip_suffix("\"]")?.split_once("[\"")?;
            let (id, _) = label.split_once("<br>")?;
            Some(id)
        })
        .collect();
    let mut nodes = Vec::new();
    let mut corners = HashMap::new();
    for (row, line) in text.lines.iter().enumerate() {
        let plain: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let mut start = 0usize;
        for segment in plain.split('│') {
            let end = start + Span::raw(segment).width();
            let id = segment.trim();
            // Leaf puts each node ID immediately below the top edge of its box.
            // Item names and edge labels can also contain IDs.
            if ids.contains(id)
                && start > 0
                && corners.get(&(start - 1)) == Some(&"┌")
                && corners.get(&end) == Some(&"┐")
            {
                let padding = &segment[..segment.len() - segment.trim_start().len()];
                let first = start + Span::raw(padding).width();
                nodes.push(MermaidNode {
                    id: id.to_owned(),
                    row,
                    columns: first..first + Span::raw(id).width(),
                });
            }
            start = end + 1;
        }
        corners.clear();
        let mut column = 0;
        for grapheme in line.styled_graphemes(text.style) {
            if matches!(grapheme.symbol, "┌" | "┐") {
                corners.insert(column, grapheme.symbol);
            }
            column += Span::raw(grapheme.symbol).width();
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_links_use_node_id_rows_and_terminal_columns() {
        let source = "flowchart BT\n    ID-1[\"ID-1<br>ID-2\"]\n    ID-2[\"ID-2<br>Elsewhere\"]";
        let text = Text::from(vec![
            Line::from("界 ┌─────┐"),
            Line::from(vec![Span::raw("界 │ "), Span::raw("ID-"), Span::raw("1│")]),
            Line::from("界 │ ID-2│"),
            Line::from("界 └─────┘"),
            Line::from("ID-2"),
        ]);

        let nodes = mermaid_nodes(source, &text);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "ID-1");
        assert_eq!(nodes[0].row, 1);
        assert_eq!(nodes[0].columns, 5..9);
    }
}

fn render_with_leaf(markdown: &str, width: u16) -> Result<Text<'static>> {
    let mut child = Command::new("leaf")
        .args(["--inline", &format!("ansi:{width}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("leaf を起動できません。PATH を確認してください")?;
    let mut stdin = child.stdin.take().context("leaf の標準入力がありません")?;
    let sent = stdin.write_all(markdown.as_bytes());
    drop(stdin);
    let output = child
        .wait_with_output()
        .context("leaf の実行に失敗しました")?;
    if !output.status.success() {
        bail!(
            "leaf の実行に失敗しました（{}）\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    sent.context("leaf に本文を送信できませんでした")?;
    output
        .stdout
        .into_text()
        .context("leaf の出力を読み取れませんでした")
}
