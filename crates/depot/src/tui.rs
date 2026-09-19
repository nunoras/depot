use std::io::{Stdout, Write, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, poll};
use crossterm::style::{Color, Print, SetForegroundColor};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{ExecutableCommand, cursor};

use depotd::{
    DepotHome, Error, Project, ProjectState, StatusSelection, Store, TaskState, role_name,
    select_project, state_name,
};

const REFRESH: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

pub fn run(home: &DepotHome, selection: &StatusSelection) -> Result<(), Error> {
    let mut terminal = Terminal::enter()?;
    loop {
        let projects = collect(home, selection)?;
        terminal.draw(&projects)?;
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

fn collect(
    home: &DepotHome,
    selection: &StatusSelection,
) -> Result<Vec<(Project, ProjectState)>, Error> {
    let store = Store::open(home)?;
    let projects: Vec<Project> = match selection {
        StatusSelection::All => store.projects()?,
        StatusSelection::Project(name) => vec![select_project(&store, Some(name))?],
        StatusSelection::CurrentDirectory => vec![select_project(&store, None)?],
    };
    projects
        .into_iter()
        .map(|project| {
            let state = store.project_state(&project)?;
            Ok((project, state))
        })
        .collect()
}

enum Segment {
    Text(String),
    State(String, TaskState),
    Dim(String),
}

fn frame(projects: &[(Project, ProjectState)], width: usize) -> Vec<Vec<Segment>> {
    let mut lines = Vec::new();
    lines.push(vec![Segment::Dim(clock())]);
    lines.push(vec![Segment::Text(String::new())]);
    if projects.is_empty() {
        lines.push(vec![Segment::Text("No projects registered.".to_string())]);
        return lines;
    }
    for (project, state) in projects {
        lines.push(vec![Segment::Text(project.slug.clone())]);
        if state.tasks.is_empty() {
            lines.push(vec![Segment::Dim("  no tasks".to_string())]);
        }
        for task in state.tasks.values() {
            let id = format!("  {:<5}", task.id.as_str());
            let status = pad(state_name(task.state), 10);
            let role = pad(role_name(task.role), 7);
            let used = id.chars().count() + status.chars().count() + role.chars().count();
            let title: String = task
                .title
                .chars()
                .take(width.saturating_sub(used + 1))
                .collect();
            lines.push(vec![
                Segment::Text(id),
                Segment::State(status, task.state),
                Segment::Text(role),
                Segment::Text(title),
            ]);
        }
        lines.push(vec![Segment::Text(String::new())]);
    }
    lines.pop();
    lines
}

fn pad(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let padding = width - out.chars().count();
    out.push_str(&" ".repeat(padding));
    out
}

fn clock() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day = seconds % 86400;
    format!(
        "depot  {:02}:{:02}:{:02} UTC  refresh 2s  q quit",
        day / 3600,
        (day % 3600) / 60,
        day % 60
    )
}

fn state_color(state: TaskState) -> Color {
    match state {
        TaskState::Landed => Color::DarkGreen,
        TaskState::Failed => Color::Red,
        TaskState::Cancelled => Color::DarkGrey,
        state if state.in_flight() => Color::Cyan,
        TaskState::Validated | TaskState::PrOpen => Color::Yellow,
        _ => Color::White,
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

    fn draw(&mut self, projects: &[(Project, ProjectState)]) -> Result<(), Error> {
        let (width, height) = size()
            .map(|(width, height)| (width as usize, height as usize))
            .unwrap_or((80, 24));
        let lines = frame(projects, width.saturating_sub(1));
        self.stdout.execute(Clear(ClearType::All))?;
        self.stdout.execute(cursor::MoveTo(0, 0))?;
        for line in lines.iter().take(height.saturating_sub(1)) {
            for segment in line {
                match segment {
                    Segment::Text(text) => {
                        self.stdout.execute(Print(text))?;
                    }
                    Segment::Dim(text) => {
                        self.stdout.execute(SetForegroundColor(Color::DarkGrey))?;
                        self.stdout.execute(Print(text))?;
                        self.stdout.execute(SetForegroundColor(Color::Reset))?;
                    }
                    Segment::State(text, state) => {
                        self.stdout
                            .execute(SetForegroundColor(state_color(*state)))?;
                        self.stdout.execute(Print(text))?;
                        self.stdout.execute(SetForegroundColor(Color::Reset))?;
                    }
                }
            }
            self.stdout.execute(Print("\r\n"))?;
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
