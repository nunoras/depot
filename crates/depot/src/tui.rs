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

use depotd::{
    DepotHome, Error, Project, ProjectState, StatusSelection, Store, Task, TaskState, role_name,
    select_project, state_name,
};

const REFRESH: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);
const WHEEL: usize = 3;

pub fn run(home: &DepotHome, selection: &StatusSelection) -> Result<(), Error> {
    let mut terminal = Terminal::enter()?;
    let mut scroll = 0;
    let mut history = false;
    loop {
        let projects = collect(home, selection)?;
        terminal.draw(&projects, scroll, history)?;
        let viewport = terminal.viewport_height();
        let deadline = std::time::Instant::now() + REFRESH;
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

fn frame(projects: &[(Project, ProjectState)], width: usize, history: bool) -> Vec<Line> {
    let mut lines = Vec::new();
    lines.push(pinned(vec![Segment::Dim(clock())]));
    let mut counts: BTreeMap<TaskState, usize> = BTreeMap::new();
    for (_, state) in projects {
        for task in state.tasks.values() {
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
    for (project, state) in projects {
        let waiting: Vec<&Task> = ordered_tasks(state)
            .into_iter()
            .filter(|task| task.state == TaskState::WaitingOnQuestion)
            .collect();
        if !waiting.is_empty() {
            lines.push(pinned(vec![Segment::Text(project.slug.clone())]));
            for task in waiting {
                push_waiting_block(&mut lines, task, width);
            }
            lines.push(pinned(vec![Segment::Text(String::new())]));
        }
        lines.push(body(vec![Segment::Text(project.slug.clone())]));
        if state.tasks.is_empty() {
            lines.push(body(vec![Segment::Dim("  no tasks".to_string())]));
        }
        for task in ordered_tasks(state) {
            if task.state == TaskState::WaitingOnQuestion {
                continue;
            }
            if !history && is_terminal(task.state) {
                continue;
            }
            lines.push(body(task_line(task, width)));
        }
        lines.push(body(vec![Segment::Text(String::new())]));
    }
    lines.pop();
    lines
}

fn task_line(task: &Task, width: usize) -> Vec<Segment> {
    let id = format!("  {:<5}", task.id.as_str());
    let status = pad(state_name(task.state), 10);
    let role = pad(role_name(task.role), 7);
    let used = id.chars().count() + status.chars().count() + role.chars().count();
    let title: String = task
        .title
        .chars()
        .take(width.saturating_sub(used + 1))
        .collect();
    vec![
        Segment::Text(id),
        Segment::State(status, task.state),
        Segment::Text(role),
        Segment::Text(title),
    ]
}

fn push_waiting_block(lines: &mut Vec<Line>, task: &Task, width: usize) {
    let mut segments = vec![Segment::State("  needs you".to_string(), task.state)];
    segments.extend(task_line(task, width));
    lines.push(pinned(segments));
    if let Some(question) = waiting_question(task) {
        let text: String = question.chars().take(width.saturating_sub(4)).collect();
        lines.push(pinned(vec![Segment::Text(format!("    {text}"))]));
    }
    for artifact in &task.artifacts {
        let path: String = artifact
            .path
            .chars()
            .take(width.saturating_sub(16))
            .collect();
        lines.push(pinned(vec![Segment::Dim(format!("    artifact {path}"))]));
    }
}

fn is_terminal(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Landed | TaskState::Failed | TaskState::Cancelled
    )
}

fn ordered_tasks(state: &ProjectState) -> Vec<&Task> {
    let mut tasks: Vec<&Task> = state.tasks.values().collect();
    tasks.sort_by_key(|task| task.state != TaskState::WaitingOnQuestion);
    tasks
}

fn waiting_question(task: &Task) -> Option<&str> {
    task.questions
        .iter()
        .rev()
        .find(|question| question.answer.is_none())
        .map(|question| question.text.as_str())
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
        projects: &[(Project, ProjectState)],
        scroll: usize,
        history: bool,
    ) -> Result<(), Error> {
        let (width, height) = size()
            .map(|(width, height)| (width as usize, height as usize))
            .unwrap_or((80, 24));
        let lines = frame(projects, width.saturating_sub(1), history);
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
    use depot_core::{Limits, ProjectId, Role, TaskId, Timestamp};
    use depotd::LocationKind;

    use super::*;

    fn at(millis: u64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn task(id: &str, state: TaskState) -> Task {
        Task {
            id: TaskId::from(id),
            project: ProjectId::from("depot"),
            title: format!("task {id}"),
            intent: String::new(),
            role: Role::Build,
            dispatch_profile: None,
            state,
            dependencies: Vec::new(),
            base_dependency: None,
            attempts: Vec::new(),
            questions: Vec::new(),
            validations: Vec::new(),
            submission: None,
            artifacts: Vec::new(),
            links: Vec::new(),
            branch_head: None,
            merge_refused: None,
            acknowledged_at: None,
            hold_pr: false,
            retry: None,
            created_at: at(0),
            updated_at: at(0),
        }
    }

    fn project_state(tasks: Vec<Task>) -> ProjectState {
        ProjectState {
            project: ProjectId::from("depot"),
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            coordinator: None,
            profiles: Default::default(),
            fallback_profiles: Vec::new(),
            limits: Limits::default(),
            always_relay_questions: false,
            auto_merge: false,
        }
    }

    fn project() -> Project {
        Project {
            id: ProjectId::from("depot"),
            kind: LocationKind::Path,
            slug: "depot".to_string(),
            created_at: at(0),
        }
    }

    fn text(line: &Line) -> String {
        line.segments
            .iter()
            .map(|segment| match segment {
                Segment::Text(text) | Segment::State(text, _) | Segment::Dim(text) => text.as_str(),
            })
            .collect()
    }

    #[test]
    fn waiting_task_is_pinned_and_absent_from_the_scrolling_body() {
        let waiting = task("t-1", TaskState::WaitingOnQuestion);
        let running = task("t-2", TaskState::Running);
        let lines = frame(
            &[(project(), project_state(vec![waiting, running]))],
            80,
            false,
        );
        let pinned: Vec<&Line> = lines.iter().filter(|line| line.pinned).collect();
        let body: Vec<&Line> = lines.iter().filter(|line| !line.pinned).collect();
        assert!(pinned.iter().any(|line| text(line).contains("needs you")));
        assert!(pinned.iter().any(|line| text(line).contains("t-1")));
        assert!(!body.iter().any(|line| text(line).contains("t-1")));
        assert!(body.iter().any(|line| text(line).contains("t-2")));
    }

    #[test]
    fn unanswered_question_rides_with_the_pinned_block() {
        let mut waiting = task("t-1", TaskState::WaitingOnQuestion);
        waiting.questions.push(depot_core::Question {
            text: "which base branch".to_string(),
            asked_at: at(0),
            answer: None,
        });
        let lines = frame(&[(project(), project_state(vec![waiting]))], 80, false);
        let pinned: Vec<&Line> = lines.iter().filter(|line| line.pinned).collect();
        assert!(
            pinned
                .iter()
                .any(|line| text(line).contains("which base branch"))
        );
    }
}
