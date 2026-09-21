use std::io::Read;
use std::time::Duration;

mod tui;

use depotd::adapters::process::Program;
use depotd::adapters::worktrees::Treehouse;
use depotd::{
    DepotHome, Error, StatusSelection, TaskRequest, acknowledge_task, add_artifact, add_project,
    add_task, answer_question, approve_tasks, ask_question, read_inbox, redirect_task,
    release_task, render_projects, render_status, restart_daemon, retry_task, rework_task,
    stop_task, submit_task, wait_for_task, write_narrative,
};

const USAGE: &str = "\
depot - coordinate a project's agent work

USAGE
  depot project add <path-or-url>
  depot project list
  depot status [--project <name>] [--all] [--history] [--tui]
  depot daemon restart [--timeout <seconds>]
  depot task add --title <title> --intent <intent> [--role <plan|build|review|fix>]
                 [--kind <plan|build|review|fix>]
                 [--depends-on <task>@<commit>]...
                 [--base-dependency <task-id>] [--hold-pr] [--project <name>]
  depot task approve <task-id>... [--project <name>]
  depot task release <task-id> [--project <name>]
  depot task answer <task-id> --text <answer> [--by <coordinator|user>] [--project <name>]
  depot task stop <task-id> [--project <name>]
  depot task acknowledge <task-id> [--project <name>]
  depot task retry <task-id> [--project <name>]
  depot task redirect <task-id> --text <direction> [--queue] [--project <name>]
  depot task rework <task-id> --text <findings> [--project <name>]
  depot task wait <task-id> [--timeout <seconds>] [--project <name>]
  depot ask --task <task-id> --project <name> [--relay] <question>
  depot submit --task <task-id> --project <name>
  depot inbox [--project <name>]
  depot artifact add <file>
  depot doc write <name> --content <text|-> [--project <name>]
  depot --version

SELECTION
  A task id may be qualified as `<slug>/<task-id>`, which works from any directory.
  Without a slug or `--project`, a bare id must resolve to exactly one project.
  No --project reads the project this directory belongs to: a project repository or its store.
  --all lists every registered project.

NOTES
  One depot daemon runs per depot home. `depotd --project` narrows it to one project and is a
  debugging flag: while it runs, no daemon drives any other registered project, and `depot status`
  says so next to the affected projects.
  A task lands held. Approving it is what lets it run.
  A role resolves to a profile through the project's machine-local .depot.toml; an unmapped role is refused.
  `--kind` is an alias for `--role` on `task add`; passing both with different values is refused.
  Multiple --depends-on need --base-dependency naming one of those tasks as the baseline.
  `task wait` blocks until the task is pr_open, landed, failed, cancelled or waiting_on_question,
  then prints where it stands. Without --timeout it waits forever; it never reads the inbox.
  `--content -` reads a document from standard input.
  `artifact add` copies the file into <depot home>/artifacts and runs the [artifacts] publish
  command from <depot home>/config.toml with DEPOT_ARTIFACT_PATH set; it prints the staged path
  and the one URL that command returned.
  `daemon restart` stops the running daemon by pid and starts the installed depotd again with the
  same project scope, appending its log to <depot home>/depotd.log.
  Landed, failed and cancelled tasks are history; `--history` shows them.
  `task redirect` refuses when the worker's current turn has ended unless `--queue` is passed;
  the daemon delivers a queued or redirected direction when the worker's next turn starts.
  `task retry` sends a failed or cancelled task back to the approved queue; a worktree the task
  still leases is reused for the new attempt.
  `task rework` files a fix task on an open pull request's branch and worktree; the original
  task cannot auto-merge until the rework validates and lands on the same pull request.
";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&arguments) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(Failure::Usage(message)) => {
            eprintln!("{message}");
            eprintln!();
            eprint!("{USAGE}");
            2
        }
        Err(Failure::Failed(error)) => {
            eprintln!("depot: {error}");
            1
        }
    }
}

enum Failure {
    Usage(String),
    Failed(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure::Failed(error)
    }
}

fn dispatch(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => Ok(USAGE.to_string()),
        Some("--version") | Some("-V") => Ok(format!("{}\n", depotd::version_line("depot"))),
        Some("project") => {
            require_coordinator()?;
            project_command(&arguments[1..])
        }
        Some("task") => {
            require_coordinator()?;
            task_command(&arguments[1..])
        }
        Some("ask") => ask_command(&arguments[1..]),
        Some("submit") => submit_command(&arguments[1..]),
        Some("doc") => doc_command(&arguments[1..]),
        Some("inbox") => {
            require_coordinator()?;
            inbox_command(&arguments[1..])
        }
        Some("artifact") => {
            require_coordinator()?;
            artifact_command(&arguments[1..])
        }
        Some("daemon") => {
            require_coordinator()?;
            daemon_command(&arguments[1..])
        }
        Some("status") => status_command(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown command `{other}`"))),
    }
}

fn require_coordinator() -> Result<(), Failure> {
    if ["DEPOT_TASK_ID", "DEPOT_ATTEMPT_ID"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return Err(Error::Project(
            "worker context may only use `depot ask` and `depot submit` to move state".to_string(),
        )
        .into());
    }
    Ok(())
}

fn project_command(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        Some("add") => project_add(&arguments[1..]),
        Some("list") => project_list(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown project command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot project` needs a subcommand: try `depot project add <path-or-url>` or `depot project list`"
                .to_string(),
        )),
    }
}

fn project_list(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&[])?;
    flags.reject_positionals()?;
    let home = DepotHome::resolve()?;
    Ok(render_projects(&home)?)
}

fn project_add(arguments: &[String]) -> Result<String, Failure> {
    let target = arguments
        .first()
        .ok_or_else(|| Failure::Usage("`depot project add` needs a path or URL".to_string()))?;
    if let Some(unexpected) = arguments.get(1) {
        return Err(Failure::Usage(format!(
            "unexpected argument `{unexpected}`"
        )));
    }

    let home = DepotHome::resolve()?;
    let added = add_project(&home, target)?;
    let mut out = String::new();
    if added.created {
        out.push_str(&format!("registered {}\n", added.project.id));
    } else {
        out.push_str(&format!("already registered {}\n", added.project.id));
    }
    out.push_str(&format!("home: {}\n", added.home.root().display()));
    if added.ignored_config {
        out.push_str("config: .depot.toml stays machine-local, ignored via .git/info/exclude\n");
    }
    Ok(out)
}

fn task_command(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        Some("add") => task_add(&arguments[1..]),
        Some("approve") => task_approve(&arguments[1..]),
        Some("answer") => task_answer(&arguments[1..]),
        Some("stop") => task_stop(&arguments[1..]),
        Some("acknowledge") => task_acknowledge(&arguments[1..]),
        Some("retry") => task_retry(&arguments[1..]),
        Some("redirect") => task_redirect(&arguments[1..]),
        Some("rework") => task_rework(&arguments[1..]),
        Some("release") => task_release(&arguments[1..]),
        Some("wait") => task_wait(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown task command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot task` needs a subcommand: add, approve, answer, stop, retry, rework, release or wait"
                .to_string(),
        )),
    }
}

fn task_add(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &["hold-pr"])?;
    flags.reject_unknown(&[
        "title",
        "intent",
        "role",
        "kind",
        "depends-on",
        "base-dependency",
        "hold-pr",
        "project",
    ])?;
    flags.reject_positionals()?;

    let request = TaskRequest {
        title: flags.required("title")?.to_string(),
        intent: flags.required("intent")?.to_string(),
        role: resolved_role(&flags)?,
        dependencies: flags.all("depends-on"),
        base_dependency: flags.value("base-dependency").map(str::to_string),
        hold_pr: flags.has("hold-pr"),
    };
    let home = DepotHome::resolve()?;
    let task = add_task(&home, flags.value("project"), &request)?;
    Ok(format!("added {}\n", task.id))
}

fn resolved_role(flags: &Flags) -> Result<String, Failure> {
    match (flags.value("role"), flags.value("kind")) {
        (Some(role), Some(kind)) if role != kind => Err(Failure::Usage(format!(
            "`--role {role}` and `--kind {kind}` disagree: pass one role, or pass both with the same value"
        ))),
        (Some(role), _) => Ok(role.to_string()),
        (None, Some(kind)) => Ok(kind.to_string()),
        (None, None) => Ok(String::new()),
    }
}

fn task_approve(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    let ids = flags.positionals();
    if ids.is_empty() {
        return Err(Failure::Usage(
            "`depot task approve` needs at least one task id".to_string(),
        ));
    }

    let home = DepotHome::resolve()?;
    let approved = approve_tasks(&home, flags.value("project"), ids)?;
    let mut out = String::new();
    for task in approved {
        out.push_str(&format!("approved {}\n", task.id));
    }
    Ok(out)
}

fn task_answer(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["text", "by", "project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task answer` needs exactly one task id".to_string(),
        ));
    }
    let text = flags.required("text")?;
    let by = flags.value("by").unwrap_or("coordinator");

    let home = DepotHome::resolve()?;
    let task = answer_question(&home, flags.value("project"), &ids[0], text, by)?;
    Ok(format!("answered {}\n", task.id))
}

fn task_stop(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task stop` needs exactly one task id".to_string(),
        ));
    }

    let home = DepotHome::resolve()?;
    let task = stop_task(&home, flags.value("project"), &ids[0])?;
    Ok(format!("stopped {}\n", task.id))
}

fn task_acknowledge(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task acknowledge` needs exactly one task id".to_string(),
        ));
    }

    let home = DepotHome::resolve()?;
    let task = acknowledge_task(&home, flags.value("project"), &ids[0])?;
    Ok(format!("acknowledged {}\n", task.id))
}

fn task_retry(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task retry` needs exactly one task id".to_string(),
        ));
    }

    let home = DepotHome::resolve()?;
    let task = retry_task(&home, flags.value("project"), &ids[0])?;
    Ok(format!("retried {}\n", task.id))
}

fn task_redirect(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &["queue"])?;
    flags.reject_unknown(&["text", "project", "queue"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task redirect` needs exactly one task id".to_string(),
        ));
    }
    let text = flags.required("text")?;

    let home = DepotHome::resolve()?;
    let (task, turn_running) = redirect_task(
        &home,
        flags.value("project"),
        &ids[0],
        text,
        flags.has("queue"),
    )?;
    if turn_running {
        Ok(format!("redirected {}\n", task.id))
    } else {
        Ok("queued, not yet delivered\n".to_string())
    }
}

fn task_rework(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["text", "project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task rework` needs exactly one task id".to_string(),
        ));
    }
    let text = flags.required("text")?;

    let home = DepotHome::resolve()?;
    let original = rework_task(&home, flags.value("project"), &ids[0], text)?;
    Ok(format!("rework filed against {}\n", original.id))
}

fn task_release(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task release` needs exactly one task id".to_string(),
        ));
    }

    let home = DepotHome::resolve()?;
    let task = release_task(&home, flags.value("project"), &ids[0])?;
    Ok(format!("released {}\n", task.id))
}

fn task_wait(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["timeout", "project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task wait` needs exactly one task id".to_string(),
        ));
    }
    let seconds = match flags.value("timeout") {
        Some(value) => value.parse::<u64>().map_err(|_| {
            Failure::Usage(format!(
                "`--timeout` needs a whole number of seconds, not `{value}`"
            ))
        })?,
        None => 0,
    };
    let home = DepotHome::resolve()?;
    let waited = wait_for_task(
        &home,
        flags.value("project"),
        &ids[0],
        (seconds > 0).then(|| Duration::from_secs(seconds)),
        depotd::MIN_WAIT_POLL,
    )?;
    if waited.timed_out {
        return Err(Error::Project(format!(
            "task `{}` is still {} after {seconds}s: pass a larger --timeout, or read where it stands with `depot status`",
            waited.task.id,
            depotd::state_name(waited.task.state)
        ))
        .into());
    }
    let mut out = format!(
        "{} {}\n",
        waited.task.id,
        depotd::state_name(waited.task.state)
    );
    if let Some(question) = waited
        .task
        .questions
        .iter()
        .rev()
        .find(|question| question.answer.is_none())
    {
        out.push_str(&format!("{}\n", question.text));
    }
    Ok(out)
}

fn artifact_command(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        Some("add") => artifact_add(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!(
            "unknown artifact command `{other}`"
        ))),
        None => Err(Failure::Usage(
            "`depot artifact` needs a subcommand: try `depot artifact add <file>`".to_string(),
        )),
    }
}

fn artifact_add(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&[])?;
    let files = flags.positionals();
    if files.len() != 1 {
        return Err(Failure::Usage(
            "`depot artifact add` needs exactly one file".to_string(),
        ));
    }
    let home = DepotHome::resolve()?;
    let staged = add_artifact(&home, std::path::Path::new(&files[0]))?;
    Ok(format!(
        "staged {}\n{}\n",
        staged.path.display(),
        staged.url
    ))
}

fn daemon_command(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        Some("restart") => daemon_restart(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown daemon command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot daemon` needs a subcommand: try `depot daemon restart`".to_string(),
        )),
    }
}

fn daemon_restart(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["timeout"])?;
    flags.reject_positionals()?;
    let home = DepotHome::resolve()?;
    let timeout = match flags.value("timeout") {
        Some(value) => value.parse::<u64>().map_err(|_| {
            Failure::Usage(format!(
                "`--timeout` needs a whole number of seconds, not `{value}`"
            ))
        })?,
        None => 0,
    };
    let timeout = if timeout > 0 {
        Duration::from_secs(timeout)
    } else {
        home.load_settings()?
            .poll_interval()
            .saturating_mul(2)
            .saturating_add(Duration::from_secs(10))
    };
    let restarted = restart_daemon(
        &home,
        &depotd::RestartOptions {
            program: depotd::installed_daemon()?,
            timeout,
            takeover_timeout: timeout,
        },
    )?;
    let mut out = String::new();
    match restarted.stopped {
        Some(pid) => out.push_str(&format!("stopped depotd pid {pid}\n")),
        None => out.push_str("no daemon was running\n"),
    }
    if restarted.projects.is_empty() {
        out.push_str(&format!(
            "started depotd pid {} covering all registered projects\n",
            restarted.pid
        ));
    } else {
        out.push_str(&format!(
            "started depotd pid {} covering {}\n",
            restarted.pid,
            restarted.projects.join(", ")
        ));
    }
    out.push_str(&format!("log: {}\n", restarted.log.display()));
    Ok(out)
}

fn ask_command(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &["relay"])?;
    flags.reject_unknown(&["task", "project", "relay"])?;
    let questions = flags.positionals();
    if questions.len() != 1 {
        return Err(Failure::Usage(
            "`depot ask` needs exactly one question".to_string(),
        ));
    }
    let home = DepotHome::resolve()?;
    let task = ask_question(
        &home,
        flags.value("project"),
        flags.required("task")?,
        &questions[0],
        flags.has("relay"),
    )?;
    Ok(format!("asked {}\n", task.id))
}

fn submit_command(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["task", "project"])?;
    flags.reject_positionals()?;
    let home = DepotHome::resolve()?;
    let worktrees = Treehouse::new(Program::new("treehouse"));
    let task = submit_task(
        &home,
        flags.value("project"),
        flags.required("task")?,
        &worktrees,
    )?;
    Ok(format!("submitted {}\n", task.id))
}

fn doc_command(arguments: &[String]) -> Result<String, Failure> {
    match arguments.first().map(String::as_str) {
        Some("write") => doc_write(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown doc command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot doc` needs a subcommand: try `depot doc write <name> --content <text|->`"
                .to_string(),
        )),
    }
}

fn doc_write(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["content", "project"])?;
    let names = flags.positionals();
    if names.len() != 1 {
        return Err(Failure::Usage(
            "`depot doc write` needs exactly one document name".to_string(),
        ));
    }
    let content = match flags.value("content") {
        Some("-") => read_stdin()?,
        Some(text) => text.to_string(),
        None => {
            return Err(Failure::Usage(
                "`depot doc write` needs `--content <text|->`".to_string(),
            ));
        }
    };

    let home = DepotHome::resolve()?;
    let path = write_narrative(&home, flags.value("project"), &names[0], &content)?;
    Ok(format!("wrote {}\n", path.display()))
}

fn inbox_command(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["project"])?;
    flags.reject_positionals()?;

    let home = DepotHome::resolve()?;
    read_inbox(&home, flags.value("project")).map_err(Failure::from)
}

fn status_command(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &["all", "tui", "history"])?;
    flags.reject_unknown(&["all", "project", "tui", "history"])?;
    flags.reject_positionals()?;

    let selection = match (flags.has("all"), flags.value("project")) {
        (true, Some(_)) => {
            return Err(Failure::Usage(
                "choose one of `--project` or `--all`, not both".to_string(),
            ));
        }
        (true, None) => StatusSelection::All,
        (false, Some(name)) => StatusSelection::Project(name.to_string()),
        (false, None) => StatusSelection::CurrentDirectory,
    };

    let home = DepotHome::resolve()?;
    if flags.has("tui") {
        tui::run(&home, &selection)?;
        return Ok(String::new());
    }
    Ok(render_status(&home, &selection, flags.has("history"))?)
}

fn read_stdin() -> Result<String, Failure> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| Failure::Failed(Error::Io(error)))?;
    Ok(text)
}

struct Flags {
    values: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

impl Flags {
    fn parse(arguments: &[String], booleans: &[&str]) -> Result<Self, Failure> {
        let mut values = Vec::new();
        let mut rest = Vec::new();
        let mut index = 0;

        while index < arguments.len() {
            let argument = arguments[index].as_str();
            let Some(flag) = argument.strip_prefix("--") else {
                rest.push(argument.to_string());
                index += 1;
                continue;
            };
            let (name, inline) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (flag, None),
            };
            if booleans.contains(&name) {
                if inline.is_some() {
                    return Err(Failure::Usage(format!("`--{name}` takes no value")));
                }
                values.push((name.to_string(), None));
                index += 1;
                continue;
            }
            let value = match inline {
                Some(value) => value,
                None => {
                    index += 1;
                    arguments
                        .get(index)
                        .cloned()
                        .ok_or_else(|| Failure::Usage(format!("`--{name}` needs a value")))?
                }
            };
            values.push((name.to_string(), Some(value)));
            index += 1;
        }

        Ok(Self { values, rest })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(candidate, _)| candidate == name)
            .and_then(|(_, value)| value.as_deref())
    }

    fn required(&self, name: &str) -> Result<&str, Failure> {
        self.value(name)
            .ok_or_else(|| Failure::Usage(format!("`--{name}` is required")))
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(candidate, _)| candidate == name)
            .filter_map(|(_, value)| value.clone())
            .collect()
    }

    fn has(&self, name: &str) -> bool {
        self.values.iter().any(|(candidate, _)| candidate == name)
    }

    fn positionals(&self) -> &[String] {
        &self.rest
    }

    fn reject_unknown(&self, known: &[&str]) -> Result<(), Failure> {
        for (name, _) in &self.values {
            if !known.contains(&name.as_str()) {
                return Err(Failure::Usage(format!("unknown flag `--{name}`")));
            }
        }
        Ok(())
    }

    fn reject_positionals(&self) -> Result<(), Failure> {
        match self.rest.first() {
            Some(unexpected) => Err(Failure::Usage(format!(
                "unexpected argument `{unexpected}`"
            ))),
            None => Ok(()),
        }
    }
}
