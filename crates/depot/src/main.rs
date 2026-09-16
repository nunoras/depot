use depotd::{DepotHome, Error, StatusSelection, add_project, render_status};

const USAGE: &str = "\
depot - coordinate a project's agent work

USAGE
  depot project add <path-or-url>
  depot status [--project <name>] [--all]

STATUS SELECTION
  No flags reads the project this directory belongs to.
  --all lists every registered project.
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
        Some("project") => project_command(&arguments[1..]),
        Some("status") => status_command(&arguments[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown command `{other}`"))),
    }
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
    if let Some(config) = &added.config_path {
        out.push_str(&format!("config: {}\n", config.display()));
    }
    Ok(out)
}

fn status_command(arguments: &[String]) -> Result<String, Failure> {
    let mut selection: Option<StatusSelection> = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--all" => {
                choose(&mut selection, StatusSelection::All)?;
                index += 1;
            }
            "--project" => {
                let name = arguments.get(index + 1).ok_or_else(|| {
                    Failure::Usage("`--project` needs a project name".to_string())
                })?;
                choose(&mut selection, StatusSelection::Project(name.clone()))?;
                index += 2;
            }
            _ if argument.starts_with("--project=") => {
                let name = argument.trim_start_matches("--project=").to_string();
                choose(&mut selection, StatusSelection::Project(name))?;
                index += 1;
            }
            other => {
                return Err(Failure::Usage(format!("unexpected argument `{other}`")));
            }
        }
    }

    let home = DepotHome::resolve()?;
    let selection = selection.unwrap_or(StatusSelection::CurrentDirectory);
    Ok(render_status(&home, &selection)?)
}

fn choose(slot: &mut Option<StatusSelection>, value: StatusSelection) -> Result<(), Failure> {
    if slot.is_some() {
        return Err(Failure::Usage(
            "choose one of `--project` or `--all`, not both".to_string(),
        ));
    }
    *slot = Some(value);
    Ok(())
}
