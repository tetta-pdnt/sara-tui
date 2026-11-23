mod app;
mod markdown;
mod sara;
mod tree;
mod ui;

use std::env;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};

use app::{Action, App};
use sara::Sara;

#[derive(Parser)]
#[command(version, about = "Sara の文書とつながりを端末で見る")]
struct Args {
    #[arg(
        long,
        value_name = "DIR",
        help = "sara.toml があるディレクトリ（既定は作業ディレクトリ）"
    )]
    project: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let project = args.project.map_or_else(env::current_dir, Ok)?;
    let project = project
        .canonicalize()
        .with_context(|| format!("プロジェクトを開けません（{}）", project.display()))?;
    if !project.join("sara.toml").is_file() {
        bail!(
            "{} に sara.toml がありません。--project で場所を指定してください",
            project.display()
        );
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("Sara TUI は対話操作ができる端末で実行してください");
    }

    let mut app = App::new(Sara { project })?;
    let mut terminal = ratatui::try_init()?;
    let result = (|| -> Result<()> {
        execute!(io::stdout(), EnableBracketedPaste)?;
        loop {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match app.handle_key(key) {
                    Action::Continue => {}
                    Action::Quit => break,
                    Action::Edit => {
                        execute!(terminal.backend_mut(), DisableBracketedPaste)?;
                        ratatui::try_restore()?;
                        terminal.show_cursor()?;
                        let result = app.edit_current();
                        enable_raw_mode()?;
                        execute!(
                            terminal.backend_mut(),
                            EnterAlternateScreen,
                            EnableBracketedPaste
                        )?;
                        terminal.clear()?;
                        app.report_error(result);
                    }
                },
                Event::Paste(text) => app.paste(&text),
                _ => {}
            }
        }
        Ok(())
    })();
    let disable_paste = execute!(io::stdout(), DisableBracketedPaste);
    let restore = ratatui::try_restore();
    result?;
    disable_paste?;
    restore?;
    Ok(())
}
