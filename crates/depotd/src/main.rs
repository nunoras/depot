use std::sync::Arc;
use std::thread;

use depotd::adapters::forge::{DEFAULT_API_BASE, GitHub, resolve_credentials};
use depotd::adapters::process::Program;
use depotd::adapters::sessions::{Boxr, Sessions};
use depotd::adapters::worktrees::Treehouse;
use depotd::{
    DepotHome, EventHook, ForgeDelivery, InstanceLock, NoEventHook, ShellEventHook,
    ShellValidation, Store, Supervisor, select_project,
};

const USAGE: &str = "depotd [--project <project>]\n\n  --project narrows the daemon to one project. It is a debugging flag: the single\n  daemon lock means no other registered project is driven while it runs.\n";

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
    let filter = arguments()?;
    let home = DepotHome::resolve()?;
    let lock = InstanceLock::acquire(&home)?;
    let store = Store::open(&home)?;
    let projects = match filter {
        Some(name) => vec![select_project(&store, Some(&name))?],
        None => store.projects()?,
    };
    if projects.is_empty() {
        return Err(depotd::Error::Project(
            "no projects are registered; add one with `depot project add`".to_string(),
        ));
    }
    let credentials = resolve_credentials(&Program::new("gh"), home.root())
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let sessions = Boxr::new(Program::new("boxr"));
    sessions
        .capabilities()
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let settings = home.load_settings()?;
    let hook: Arc<dyn EventHook> = match &settings.on_event {
        Some(on_event) => Arc::new(ShellEventHook::new(on_event.command.clone())),
        None => Arc::new(NoEventHook),
    };
    let supervisor = Supervisor::new(
        &store,
        projects,
        sessions,
        Treehouse::new(Program::new("treehouse")),
        ShellValidation,
        ForgeDelivery::new(GitHub::new(DEFAULT_API_BASE, credentials.token)),
        hook,
    );
    supervisor.recover()?;
    lock.record_scope(supervisor.projects())?;
    let mut turn: usize = 0;
    loop {
        if let Err(error) = supervisor.tick(turn) {
            if error.is_lock_contention() {
                eprintln!("depotd: {error}; continuing");
            } else {
                return Err(error);
            }
        }
        lock.refresh_heartbeat()?;
        turn = turn.wrapping_add(1);
        thread::sleep(home.load_settings()?.poll_interval());
    }
}

fn arguments() -> depotd::Result<Option<String>> {
    let mut values = std::env::args().skip(1);
    let mut project = None;
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--project" => {
                project = Some(values.next().ok_or_else(|| {
                    depotd::Error::Project(format!("--project needs a value\n{USAGE}"))
                })?)
            }
            "--help" | "-h" => return Err(depotd::Error::Project(USAGE.to_string())),
            other => {
                return Err(depotd::Error::Project(format!(
                    "unknown argument `{other}`\n{USAGE}"
                )));
            }
        }
    }
    Ok(project)
}
