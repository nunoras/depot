use std::io::{Stdout, Write, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, poll};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{ExecutableCommand, cursor};

use depotd::{DepotHome, Error, StatusSelection, render_status};

const REFRESH: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

pub fn run(home: &DepotHome, selection: &StatusSelection) -> Result<(), Error> {
    let mut terminal = Terminal::enter()?;
    loop {
        let frame = render_status(home, selection)?;
        terminal.draw(&frame)?;
        let deadline = std::time::Instant::now() + REFRESH;
        while std::time::Instant::now() < deadline {
            if poll(POLL)? {
                match event::read()? {
                    Event::Resize(_, _) => break,
                    event if event.should_quit() => {
                        terminal.leave()?;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

trait ShouldQuit {
    fn should_quit(&self) -> bool;
}

impl ShouldQuit for Event {
    fn should_quit(&self) -> bool {
        match self {
            Event::Key(event) => {
                event.kind == KeyEventKind::Press
                    && (matches!(event.code, KeyCode::Char('q') | KeyCode::Esc)
                        || (event.code == KeyCode::Char('c')
                            && event.modifiers.contains(KeyModifiers::CONTROL)))
            }
            _ => false,
        }
    }
}

struct Terminal {
    stdout: Stdout,
}

impl Terminal {
    fn enter() -> Result<Self, Error> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        Ok(Self { stdout })
    }

    fn draw(&mut self, frame: &str) -> Result<(), Error> {
        let width = size().map(|(width, _)| width as usize).unwrap_or(80);
        self.stdout.execute(Clear(ClearType::All))?;
        self.stdout.execute(cursor::MoveTo(0, 0))?;
        for line in frame.lines() {
            let visible: String = line.chars().take(width.saturating_sub(1)).collect();
            writeln!(self.stdout, "{visible}")?;
        }
        self.stdout.flush()?;
        Ok(())
    }

    fn leave(&mut self) -> Result<(), Error> {
        self.stdout.execute(LeaveAlternateScreen)?;
        self.stdout.execute(cursor::Show)?;
        self.stdout.flush()?;
        disable_raw_mode()?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.stdout.execute(LeaveAlternateScreen);
        let _ = self.stdout.execute(cursor::Show);
        let _ = disable_raw_mode();
    }
}
