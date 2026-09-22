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

const USAGE: &str = "depotd [--project <project>]... [--version]\n\n  --project narrows the daemon to one project. It is a debugging flag: the single\n  daemon lock means no other registered project is driven while it runs.\n  Without --project the daemon covers every registered project.\n";

fn main() {
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("{}", depotd::version_line("depotd"));
        return;
    }
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
    let store = Store::open_migrating(&home)?;
    let projects = match filter {
        Some(names) => {
            let mut projects = Vec::new();
            for name in names {
                projects.push(select_project(&store, Some(&name))?);
            }
            projects
        }
        None => store.projects()?,
    };
    if projects.is_empty() {
        return Err(depotd::Error::Project(
            "no projects are registered; add one with `depot project add`".to_string(),
        ));
    }
    let credentials = resolve_credentials(&Program::new("gh"), &home.secrets_dir())
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
    let scope = lock.record_scope(supervisor.projects())?;
    let mut turn: usize = 0;
    loop {
        if depotd::stop_requested(&home, scope.pid, scope.started_at_millis)? {
            depotd::clear_stop_request(&home)?;
            break;
        }
        if let Err(error) = supervisor.tick(turn) {
            if error.is_project() || error.is_lock_contention() {
                eprintln!("depotd: {error}; continuing");
            } else {
                return Err(error);
            }
        }
        lock.refresh_heartbeat()?;
        turn = turn.wrapping_add(1);
        thread::sleep(home.load_settings()?.poll_interval());
    }
    Ok(())
}

fn arguments() -> depotd::Result<Option<Vec<String>>> {
    let mut values = std::env::args().skip(1);
    let mut projects = Vec::new();
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--project" => projects.push(values.next().ok_or_else(|| {
                depotd::Error::Project(format!("--project needs a value\n{USAGE}"))
            })?),
            "--help" | "-h" => return Err(depotd::Error::Project(USAGE.to_string())),
            other => {
                return Err(depotd::Error::Project(format!(
                    "unknown argument `{other}`\n{USAGE}"
                )));
            }
        }
    }
    Ok((!projects.is_empty()).then_some(projects))
}
