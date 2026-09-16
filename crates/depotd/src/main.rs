use std::thread;

use depotd::adapters::forge::{DEFAULT_API_BASE, GitHub, resolve_credentials};
use depotd::adapters::process::Program;
use depotd::adapters::sessions::{Boxr, Sessions};
use depotd::adapters::worktrees::Treehouse;
use depotd::{
    Daemon, DepotHome, ForgeDelivery, InstanceLock, ShellValidation, StderrNotifier, Store,
    select_project,
};

const USAGE: &str = "depotd --project <project>\n";

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("depotd: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> depotd::Result<()> {
    let project_name = arguments()?;
    let home = DepotHome::resolve()?;
    let _lock = InstanceLock::acquire(&home)?;
    let store = Store::open(&home)?;
    let project = select_project(&store, Some(&project_name))?;
    let config = store.project_config(&project)?;
    let credentials = resolve_credentials(&Program::new("gh"), home.root())
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let sessions = Boxr::new(Program::new("boxr"));
    sessions
        .capabilities()
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let daemon = Daemon::new(
        &store,
        project,
        sessions,
        Treehouse::new(Program::new("treehouse")),
        ShellValidation,
        ForgeDelivery::new(
            GitHub::new(DEFAULT_API_BASE, credentials.token),
            config.pull_request.base,
        ),
        StderrNotifier,
    );
    daemon.recover()?;
    loop {
        daemon.tick()?;
        thread::sleep(home.load_settings()?.poll_interval());
    }
}

fn arguments() -> depotd::Result<String> {
    let mut values = std::env::args().skip(1);
    let mut project = None;
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--project" => project = values.next(),
            "--help" | "-h" => return Err(depotd::Error::Project(USAGE.to_string())),
            other => {
                return Err(depotd::Error::Project(format!(
                    "unknown argument `{other}`\n{USAGE}"
                )));
            }
        }
    }
    project.ok_or_else(|| depotd::Error::Project(format!("--project is required\n{USAGE}")))
}
