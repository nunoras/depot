use std::io::Read;

mod tui;

use depotd::{
    DepotHome, Error, StatusSelection, TaskRequest, acknowledge_task, add_project, add_task,
    answer_question, approve_tasks, ask_question, read_inbox, redirect_task, release_task,
    render_status, stop_task, submit_task, write_narrative,
};

const USAGE: &str = "\
depot - coordinate a project's agent work

USAGE
  depot project add <path-or-url>
  depot status [--project <name>] [--all] [--history] [--tui]
  depot task add --title <title> --intent <intent> [--role <plan|build|review|fix>]
                 [--depends-on <task>@<commit>]...
                 [--base-dependency <task-id>] [--hold-pr] [--project <name>]
  depot task approve <task-id>... [--project <name>]
  depot task release <task-id> [--project <name>]
  depot task answer <task-id> --text <answer> [--by <coordinator|user>] [--project <name>]
  depot task stop <task-id> [--project <name>]
  depot task acknowledge <task-id> [--project <name>]
  depot task redirect <task-id> --text <direction> [--project <name>]
  depot ask --task <task-id> --project <name> [--relay] <question>
  depot submit --task <task-id> --project <name>
  depot inbox [--project <name>]
  depot doc write <name> --content <text|-> [--project <name>]

SELECTION
  No --project reads the project this directory belongs to: a project repository or its store.
  --all lists every registered project.

NOTES
  A task lands held. Approving it is what lets it run.
  A role resolves to a profile through the project's machine-local .depot.toml; an unmapped role is refused.
  Multiple --depends-on need --base-dependency naming one of those tasks as the baseline.
  `--content -` reads a document from standard input.
  Failed and cancelled tasks fade from the default status once a live task
  depends on them or they are acknowledged; `--history` shows them.
  `task redirect` queues a new direction for a running worker; the daemon delivers it when the
  worker's current turn ends.
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
        Some(other) => Err(Failure::Usage(format!("unknown project command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot project` needs a subcommand: try `depot project add <path-or-url>`".to_string(),
        )),
    }
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
        Some("redirect") => task_redirect(&arguments[1..]),
        Some("release") => task_release(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown task command `{other}`"))),
        None => Err(Failure::Usage(
            "`depot task` needs a subcommand: add, approve, answer, stop or release".to_string(),
        )),
    }
}

fn task_add(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &["hold-pr"])?;
    flags.reject_unknown(&[
        "title",
        "intent",
        "role",
        "depends-on",
        "base-dependency",
        "hold-pr",
        "project",
    ])?;
    flags.reject_positionals()?;

    let request = TaskRequest {
        title: flags.required("title")?.to_string(),
        intent: flags.required("intent")?.to_string(),
        role: flags.value("role").unwrap_or_default().to_string(),
        dependencies: flags.all("depends-on"),
        base_dependency: flags.value("base-dependency").map(str::to_string),
        hold_pr: flags.has("hold-pr"),
    };
    let home = DepotHome::resolve()?;
    let task = add_task(&home, flags.value("project"), &request)?;
    Ok(format!("added {}\n", task.id))
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

fn task_redirect(arguments: &[String]) -> Result<String, Failure> {
    let flags = Flags::parse(arguments, &[])?;
    flags.reject_unknown(&["text", "project"])?;
    let ids = flags.positionals();
    if ids.len() != 1 {
        return Err(Failure::Usage(
            "`depot task redirect` needs exactly one task id".to_string(),
        ));
    }
    let text = flags.required("text")?;

    let home = DepotHome::resolve()?;
    let task = redirect_task(&home, flags.value("project"), &ids[0], text)?;
    Ok(format!("redirected {}\n", task.id))
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
    let task = submit_task(&home, flags.value("project"), flags.required("task")?)?;
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
