use std::io::Write;
use std::process::{Command, Stdio};

use ansi_to_tui::IntoText;
use anyhow::{bail, Context, Result};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};

#[derive(Default)]
pub struct MarkdownPreview {
    rendered: Option<(u16, Text<'static>)>,
}

impl MarkdownPreview {
    pub fn at_width(&mut self, markdown: &str, width: u16) -> &Text<'static> {
        let width = width.max(20);
        let (rendered_width, text) = self
            .rendered
            .get_or_insert_with(|| (width, render(markdown, width)));
        if *rendered_width != width {
            *text = render(markdown, width);
            *rendered_width = width;
        }
        text
    }
}

fn render(markdown: &str, width: u16) -> Text<'static> {
    render_with_leaf(markdown, width).unwrap_or_else(|error| {
        let mut text = Text::from(vec![
            Line::styled(
                "leaf で本文を表示できません。",
                Style::new().fg(Color::LightRed),
            ),
            Line::default(),
        ]);
        text.lines.extend(Text::from(format!("{error:#}")).lines);
        text
    })
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
