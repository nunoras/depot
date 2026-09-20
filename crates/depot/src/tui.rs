use std::collections::BTreeMap;
use std::io::{Stdout, Write, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind, poll};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
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
const WHEEL: usize = 3;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn run(home: &DepotHome, selection: &StatusSelection) -> Result<(), Error> {
    let mut terminal = Terminal::enter()?;
    let mut scroll = 0;
    let mut history = false;
    let mut tick: usize = 0;
    let mut projects = collect(home, selection)?;
    let mut collected = std::time::Instant::now();
    loop {
        let viewport = terminal.viewport_height();
        terminal.draw(&projects, tick, scroll, history)?;
        let deadline = std::time::Instant::now() + TICK;
        while std::time::Instant::now() < deadline {
            if poll(POLL)? {
                match event::read()? {
                    Event::Resize(_, _) => break,
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => scroll = scroll.saturating_sub(WHEEL),
                        MouseEventKind::ScrollDown => scroll += WHEEL,
                        _ => {}
                    },
                    Event::Key(event) if event.kind == KeyEventKind::Press => match event.code {
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => scroll += 1,
                        KeyCode::PageUp => scroll = scroll.saturating_sub(viewport),
                        KeyCode::PageDown => scroll += viewport,
                        KeyCode::Char('h') => {
                            history = !history;
                            scroll = 0;
                            break;
                        }
                        KeyCode::Char('q') | KeyCode::Esc => {
                            terminal.leave()?;
                            return Ok(());
                        }
                        KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                            terminal.leave()?;
                            return Ok(());
                        }
                        _ => {}
                    },
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

#[derive(Clone)]
enum Segment {
    Text(String),
    State(String, TaskState),
    Dim(String),
}

struct Line {
    segments: Vec<Segment>,
    pinned: bool,
}

fn pinned(segments: Vec<Segment>) -> Line {
    Line {
        segments,
        pinned: true,
    }
}

fn body(segments: Vec<Segment>) -> Line {
    Line {
        segments,
        pinned: false,
    }
}

fn frame(
    projects: &[ProjectView],
    tick: usize,
    now: u64,
    width: usize,
    history: bool,
) -> Vec<Line> {
    let mut lines = Vec::new();
    lines.push(pinned(vec![Segment::Dim(clock())]));
    let mut counts: BTreeMap<TaskState, usize> = BTreeMap::new();
    for project in projects {
        for task in &project.tasks {
            if is_terminal(task.state) {
                *counts.entry(task.state).or_default() += 1;
            }
        }
    }
    if history {
        lines.push(pinned(vec![Segment::Dim("history on  h back".to_string())]));
    } else if !counts.is_empty() {
        let summary = [
            (TaskState::Landed, "landed"),
            (TaskState::Failed, "failed"),
            (TaskState::Cancelled, "cancelled"),
        ]
        .into_iter()
        .filter_map(|(task_state, label)| {
            counts
                .get(&task_state)
                .map(|count| format!("{count} {label}"))
        })
        .collect::<Vec<_>>()
        .join(" · ");
        lines.push(pinned(vec![Segment::Dim(format!(
            "{summary}  h shows history"
        ))]));
    }
    lines.push(pinned(vec![Segment::Text(String::new())]));
    if projects.is_empty() {
        lines.push(pinned(vec![Segment::Text(
            "No projects registered.".to_string(),
        )]));
        return lines;
    }
    for project in projects {
        let waiting: Vec<&TaskView> = project
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::WaitingOnQuestion)
            .collect();
        if !waiting.is_empty() {
            lines.push(pinned(vec![Segment::Text(project.slug.clone())]));
            for task in waiting {
                push_waiting_block(&mut lines, task, now, width);
            }
            lines.push(pinned(vec![Segment::Text(String::new())]));
        }
        lines.push(body(vec![Segment::Text(project.slug.clone())]));
        if project.tasks.is_empty() {
            lines.push(body(vec![Segment::Dim("  no tasks".to_string())]));
        }
        for task in &project.tasks {
            if task.state == TaskState::WaitingOnQuestion {
                continue;
            }
            if !history && is_terminal(task.state) {
                continue;
            }
            for line in task_lines(task, tick, now, width) {
                lines.push(body(line));
            }
        }
        lines.push(body(vec![Segment::Text(String::new())]));
    }
    lines.pop();
    lines
}

fn task_lines(task: &TaskView, tick: usize, now: u64, width: usize) -> Vec<Vec<Segment>> {
    let id = format!("  {:<5}", task.id.as_str());
    let status = if task.running() {
        SPINNER[tick % SPINNER.len()].to_string()
    } else {
        pad(state_name(task.state), 10)
    };
    let role = pad(role_name(task.role), 7);
    let mut lines = vec![vec![
        Segment::Text(id),
        Segment::State(status, task.state),
        Segment::Text(role),
    ]];
    let title: String = task.title.chars().take(width.saturating_sub(4)).collect();
    lines.push(vec![Segment::Text(format!("    {title}"))]);
    if task.running() {
        lines.push(vec![Segment::Dim(format!("    {}", stats(task, now)))]);
    }
    lines
}

fn push_waiting_block(lines: &mut Vec<Line>, task: &TaskView, now: u64, width: usize) {
    let rendered = task_lines(task, 0, now, width);
    let mut head = vec![Segment::State("  needs you".to_string(), task.state)];
    head.extend(rendered[0].clone());
    lines.push(pinned(head));
    for line in &rendered[1..] {
        lines.push(pinned(line.clone()));
    }
    if let Some(question) = &task.question {
        let text: String = question.chars().take(width.saturating_sub(4)).collect();
        lines.push(pinned(vec![Segment::Text(format!("    {text}"))]));
    }
    for artifact in &task.artifacts {
        let path: String = artifact.chars().take(width.saturating_sub(16)).collect();
        lines.push(pinned(vec![Segment::Dim(format!("    artifact {path}"))]));
    }
}

fn is_terminal(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Landed | TaskState::Failed | TaskState::Cancelled
    )
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
        "depot  {:02}:{:02}:{:02} UTC  refresh 2s  h history  q quit",
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

struct Terminal {
    stdout: Stdout,
}

impl Terminal {
    fn enter() -> Result<Self, Error> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        stdout.execute(EnableMouseCapture)?;
        Ok(Self { stdout })
    }

    fn viewport_height(&self) -> usize {
        size()
            .map(|(_, height)| height as usize)
            .unwrap_or(24)
            .saturating_sub(1)
    }

    fn draw(
        &mut self,
        projects: &[ProjectView],
        tick: usize,
        scroll: usize,
        history: bool,
    ) -> Result<(), Error> {
        let (width, height) = size()
            .map(|(width, height)| (width as usize, height as usize))
            .unwrap_or((80, 24));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let lines = frame(projects, tick, now, width.saturating_sub(1), history);
        let pinned: Vec<&Line> = lines.iter().filter(|line| line.pinned).collect();
        let body: Vec<&Line> = lines.iter().filter(|line| !line.pinned).collect();
        let body_window = height.saturating_sub(1).saturating_sub(pinned.len()).max(1);
        let max_scroll = body.len().saturating_sub(body_window);
        let scroll = scroll.min(max_scroll);
        self.stdout.execute(Clear(ClearType::All))?;
        self.stdout.execute(cursor::MoveTo(0, 0))?;
        let mut header = pinned[0].segments.clone();
        if max_scroll > 0 {
            header.push(Segment::Dim(format!(
                "  scroll {}/{}",
                scroll + 1,
                max_scroll + 1
            )));
        }
        self.print_line(&header)?;
        for line in pinned.iter().skip(1) {
            self.print_line(&line.segments)?;
        }
        for line in body.iter().skip(scroll).take(body_window) {
            self.print_line(&line.segments)?;
        }
        self.stdout.flush()?;
        Ok(())
    }

    fn print_line(&mut self, segments: &[Segment]) -> Result<(), Error> {
        for segment in segments {
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
        Ok(())
    }

    fn leave(&mut self) -> Result<(), Error> {
        self.stdout.execute(DisableMouseCapture)?;
        self.stdout.execute(LeaveAlternateScreen)?;
        self.stdout.execute(cursor::Show)?;
        self.stdout.flush()?;
        disable_raw_mode()?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.stdout.execute(DisableMouseCapture);
        let _ = self.stdout.execute(LeaveAlternateScreen);
        let _ = self.stdout.execute(cursor::Show);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, title: &str, state: TaskState) -> TaskView {
        TaskView {
            id: TaskId::from(id),
            state,
            role: Role::Build,
            title: title.to_string(),
            started_at: Some(Timestamp::from_millis(0)),
            steps: None,
            question: None,
            artifacts: Vec::new(),
        }
    }

    fn project_view(tasks: Vec<TaskView>) -> Vec<ProjectView> {
        vec![ProjectView {
            slug: "depot".to_string(),
            tasks,
        }]
    }

    fn split(lines: &[Line]) -> (Vec<String>, Vec<String>) {
        let render = |line: &Line| {
            line.segments
                .iter()
                .map(|segment| match segment {
                    Segment::Text(text) | Segment::State(text, _) | Segment::Dim(text) => {
                        text.as_str()
                    }
                })
                .collect::<String>()
        };
        (
            lines
                .iter()
                .filter(|line| line.pinned)
                .map(render)
                .collect(),
            lines
                .iter()
                .filter(|line| !line.pinned)
                .map(render)
                .collect(),
        )
    }

    #[test]
    fn running_task_renders_spinner_title_and_stats() {
        let projects = project_view(vec![view(
            "t-1",
            "Rework the status TUI task layout",
            TaskState::Running,
        )]);
        let lines = frame(&projects, 3, 130_000, 80, false);
        assert!(
            matches!(&lines[3].segments[1], Segment::State(marker, TaskState::Running) if *marker == SPINNER[3])
        );
        assert!(
            matches!(&lines[4].segments[0], Segment::Text(text) if text == "    Rework the status TUI task layout")
        );
        assert!(matches!(&lines[5].segments[0], Segment::Dim(text) if text == "    up 2m10s"));
    }

    #[test]
    fn held_task_keeps_two_lines_without_stats() {
        let projects = project_view(vec![view(
            "t-1",
            "Rework the status TUI task layout",
            TaskState::WaitingOnQuestion,
        )]);
        let lines = frame(&projects, 0, 0, 80, false);
        assert_eq!(lines.len(), 7);
        assert!(matches!(&lines[3].segments[0], Segment::State(text, _) if text == "  needs you"));
        assert!(
            matches!(&lines[4].segments[0], Segment::Text(text) if text.starts_with("    Rework"))
        );
    }

    #[test]
    fn running_task_reports_session_steps_when_exposed() {
        let mut task = view("t-1", "task t-1", TaskState::Running);
        task.steps = Some(42);
        let projects = project_view(vec![task]);
        let lines = frame(&projects, 0, 130_000, 80, false);
        assert!(
            matches!(&lines[5].segments[0], Segment::Dim(text) if text == "    up 2m10s  42 steps")
        );
    }

    #[test]
    fn elapsed_formats_by_magnitude() {
        assert_eq!(format_elapsed(42_000), "42s");
        assert_eq!(format_elapsed(130_000), "2m10s");
        assert_eq!(format_elapsed(3_725_000), "1h02m");
    }

    #[test]
    fn waiting_task_is_pinned_and_absent_from_the_scrolling_body() {
        let waiting = view("t-1", "task t-1", TaskState::WaitingOnQuestion);
        let running = view("t-2", "task t-2", TaskState::Running);
        let lines = frame(&project_view(vec![waiting, running]), 0, 0, 80, false);
        let (pinned, body) = split(&lines);
        assert!(pinned.iter().any(|line| line.contains("needs you")));
        assert!(pinned.iter().any(|line| line.contains("t-1")));
        assert!(!body.iter().any(|line| line.contains("t-1")));
        assert!(body.iter().any(|line| line.contains("t-2")));
    }

    #[test]
    fn unanswered_question_rides_with_the_pinned_block() {
        let mut waiting = view("t-1", "task t-1", TaskState::WaitingOnQuestion);
        waiting.question = Some("which base branch".to_string());
        let lines = frame(&project_view(vec![waiting]), 0, 0, 80, false);
        let (pinned, _) = split(&lines);
        assert!(pinned.iter().any(|line| line.contains("which base branch")));
    }

    #[test]
    fn terminal_tasks_hide_until_history_opens() {
        let landed = view("t-1", "task t-1", TaskState::Landed);
        let running = view("t-2", "task t-2", TaskState::Running);
        let projects = project_view(vec![landed, running]);
        let (pinned, body) = split(&frame(&projects, 0, 0, 80, false));
        assert!(!body.iter().any(|line| line.contains("t-1")));
        assert!(pinned.iter().any(|line| line.contains("1 landed")));
        let (_, body) = split(&frame(&projects, 0, 0, 80, true));
        assert!(body.iter().any(|line| line.contains("t-1")));
    }
}
