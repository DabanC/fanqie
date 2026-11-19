//! Terminal lifecycle, event polling, and progress flush on exit.
use std::{
    io,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::{app::App, ui};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(80);
const PROGRESS_SAVE_INTERVAL: Duration = Duration::from_secs(3);

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub(crate) fn run(app: &mut App) -> Result<()> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));
    enable_raw_mode().context("无法进入终端模式，请在交互终端中启动")?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let result: Result<()> = (|| {
        let mut last_save = Instant::now();
        while !app.quit {
            app.poll_results();
            terminal.draw(|frame| ui::draw(frame, app))?;
            if event::poll(EVENT_POLL_INTERVAL)? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                    Event::Paste(text) => app.paste(&text),
                    _ => {}
                }
            }
            if last_save.elapsed() > PROGRESS_SAVE_INTERVAL {
                app.save_progress();
                last_save = Instant::now();
            }
            app.maybe_prefetch();
            app.tick = app.tick.wrapping_add(1);
        }
        Ok(())
    })();
    app.stop_loading();
    let saved = app.save_progress();
    result?;
    if !saved {
        anyhow::bail!("{}", app.status);
    }
    Ok(())
}
