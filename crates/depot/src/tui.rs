use std::io::{Stdout, Write, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, poll};
use crossterm::style::{Color, Print, SetForegroundColor};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{ExecutableCommand, cursor};

use depotd::adapters::{process::Program, toon::Document};
use depotd::{
    DepotHome, Error, Project, Role, SessionId, StatusSelection, Store, TaskId, TaskState,
    Timestamp, role_name, select_project, state_name,
};

const REFRESH: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(120);
const POLL: Duration = Duration::from_millis(100);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn run(home: &DepotHome, selection: &StatusSelection) -> Result<(), Error> {
    let mut terminal = Terminal::enter()?;
    let mut tick: usize = 0;
    let mut projects = collect(home, selection)?;
    let mut collected = std::time::Instant::now();
    loop {
        terminal.draw(&projects, tick)?;
        let deadline = std::time::Instant::now() + TICK;
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
        tick = tick.wrapping_add(1);
        if collected.elapsed() >= REFRESH {
            projects = collect(home, selection)?;
            collected = std::time::Instant::now();
        }
    }
}

struct TaskView {
    id: TaskId,
    state: TaskState,
    role: Role,
    title: String,
    started_at: Option<Timestamp>,
    steps: Option<u64>,
    question: Option<String>,
    artifacts: Vec<String>,
}

impl TaskView {
    fn running(&self) -> bool {
        self.state == TaskState::Running
    }
}

struct ProjectView {
    slug: String,
    tasks: Vec<TaskView>,
}

fn collect(home: &DepotHome, selection: &StatusSelection) -> Result<Vec<ProjectView>, Error> {
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
            let mut tasks: Vec<TaskView> = state
                .tasks
                .values()
                .map(|task| {
                    let attempt = task.attempts.last();
                    let session = attempt.and_then(|attempt| attempt.session.clone());
                    let steps = match (&session, task.state) {
                        (Some(session), TaskState::Running) => session_steps(session),
                        _ => None,
                    };
                    TaskView {
                        id: task.id.clone(),
                        state: task.state,
                        role: task.role,
                        title: task.title.clone(),
                        started_at: attempt.map(|attempt| attempt.started_at),
                        steps,
                        question: task
                            .questions
                            .iter()
                            .rev()
                            .find(|question| question.answer.is_none())
                            .map(|question| question.text.clone()),
                        artifacts: task
                            .artifacts
                            .iter()
                            .map(|artifact| artifact.path.clone())
                            .collect(),
                    }
                })
                .collect();
            tasks.sort_by_key(|task| task.state != TaskState::WaitingOnQuestion);
            Ok(ProjectView {
                slug: project.slug,
                tasks,
            })
        })
        .collect()
}

fn session_steps(session: &SessionId) -> Option<u64> {
    let output = Program::new("boxr")
        .run(&["status".to_owned(), session.as_str().to_owned()], None)
        .ok()?;
    let document = Document::parse(&output.stdout).ok()?;
    document.scalar("steps")?.trim().parse().ok()
}

#[derive(Debug)]
enum Segment {
    Text(String),
    State(String, TaskState),
    Dim(String),
}

fn frame(projects: &[ProjectView], tick: usize, now: u64, width: usize) -> Vec<Vec<Segment>> {
    let mut lines = Vec::new();
    lines.push(vec![Segment::Dim(clock())]);
    lines.push(vec![Segment::Text(String::new())]);
    if projects.is_empty() {
        lines.push(vec![Segment::Text("No projects registered.".to_string())]);
        return lines;
    }
    for project in projects {
        lines.push(vec![Segment::Text(project.slug.clone())]);
        if project.tasks.is_empty() {
            lines.push(vec![Segment::Dim("  no tasks".to_string())]);
        }
        for task in &project.tasks {
            let id = format!("  {:<5}", task.id.as_str());
            let role = pad(role_name(task.role), 7);
            let status = if task.running() {
                SPINNER[tick % SPINNER.len()].to_string()
            } else {
                pad(state_name(task.state), 10)
            };
            lines.push(vec![
                Segment::Text(id),
                Segment::State(status, task.state),
                Segment::Text(role),
            ]);
            let title: String = task.title.chars().take(width.saturating_sub(4)).collect();
            lines.push(vec![Segment::Text(format!("    {}", title))]);
            if task.running() {
                lines.push(vec![Segment::Dim(format!("    {}", stats(task, now)))]);
            }
            if task.state == TaskState::WaitingOnQuestion {
                if let Some(question) = &task.question {
                    let text: String = question.chars().take(width.saturating_sub(4)).collect();
                    lines.push(vec![Segment::Text(format!("    {text}"))]);
                }
                for artifact in &task.artifacts {
                    let path: String = artifact.chars().take(width.saturating_sub(16)).collect();
                    lines.push(vec![Segment::Dim(format!("    artifact {path}"))]);
                }
            }
        }
        lines.push(vec![Segment::Text(String::new())]);
    }
    lines.pop();
    lines
}

fn stats(task: &TaskView, now: u64) -> String {
    let elapsed = task
        .started_at
        .map(|started| format_elapsed(now.saturating_sub(started.millis())))
        .unwrap_or_else(|| "unknown".to_string());
    match task.steps {
        Some(steps) => format!("up {}  {} steps", elapsed, steps),
        None => format!("up {}", elapsed),
    }
}

fn format_elapsed(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds >= 3600 {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}s", seconds)
    }
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

    fn draw(&mut self, projects: &[ProjectView], tick: usize) -> Result<(), Error> {
        let (width, height) = size()
            .map(|(width, height)| (width as usize, height as usize))
            .unwrap_or((80, 24));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let lines = frame(projects, tick, now, width.saturating_sub(1));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn view(state: TaskState) -> TaskView {
        TaskView {
            id: TaskId::new("t-1"),
            state,
            role: Role::Build,
            title: "Rework the status TUI task layout".to_string(),
            started_at: Some(Timestamp::from_millis(0)),
            steps: None,
            question: None,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn running_task_renders_spinner_description_and_stats() {
        let projects = vec![ProjectView {
            slug: "depot".to_string(),
            tasks: vec![view(TaskState::Running)],
        }];
        let lines = frame(&projects, 3, 130_000, 80);
        assert!(
            matches!(&lines[3][1], Segment::State(marker, TaskState::Running) if *marker == SPINNER[3])
        );
        assert!(
            matches!(&lines[4][0], Segment::Text(text) if text == "    Rework the status TUI task layout")
        );
        assert!(matches!(&lines[5][0], Segment::Dim(text) if text == "    up 2m10s"));
    }

    #[test]
    fn held_task_keeps_two_lines_without_stats() {
        let projects = vec![ProjectView {
            slug: "depot".to_string(),
            tasks: vec![view(TaskState::WaitingOnQuestion)],
        }];
        let lines = frame(&projects, 0, 0, 80);
        assert_eq!(lines.len(), 5);
        assert!(
            matches!(&lines[3][1], Segment::State(text, _) if *text == pad(state_name(TaskState::WaitingOnQuestion), 10))
        );
        assert!(matches!(&lines[4][0], Segment::Text(text) if text.starts_with("    Rework")));
    }

    #[test]
    fn running_task_reports_session_steps_when_exposed() {
        let mut task = view(TaskState::Running);
        task.steps = Some(42);
        let projects = vec![ProjectView {
            slug: "depot".to_string(),
            tasks: vec![task],
        }];
        let lines = frame(&projects, 0, 130_000, 80);
        assert!(matches!(&lines[5][0], Segment::Dim(text) if text == "    up 2m10s  42 steps"));
    }

    #[test]
    fn elapsed_formats_by_magnitude() {
        assert_eq!(format_elapsed(42_000), "42s");
        assert_eq!(format_elapsed(130_000), "2m10s");
        assert_eq!(format_elapsed(3_725_000), "1h02m");
    }
}
