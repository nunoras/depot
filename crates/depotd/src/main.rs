use std::thread;

use depotd::adapters::forge::{Credentials, DEFAULT_API_BASE, GitHub, resolve_credentials};
use depotd::adapters::process::Program;
use depotd::adapters::sessions::{Boxr, Sessions};
use depotd::adapters::worktrees::Treehouse;
use depotd::{
    Daemon, DepotHome, EventHook, ForgeDelivery, InstanceLock, NoEventHook, Project, Settings,
    ShellEventHook, ShellValidation, Store, select_project,
};

const USAGE: &str = "depotd [--project <project>]\n";

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
    let _lock = InstanceLock::acquire(&home)?;
    let store = Store::open(&home)?;
    let credentials = resolve_credentials(&Program::new("gh"), home.root())
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let sessions = Boxr::new(Program::new("boxr"));
    sessions
        .capabilities()
        .map_err(|error| depotd::Error::Project(error.to_string()))?;
    let settings = home.load_settings()?;
    for project in selected_projects(&store, &filter)? {
        daemon(&store, &project, &credentials, &settings)
            .recover()
            .map_err(|error| depotd::Error::Project(format!("{}: {error}", project.slug)))?;
    }
    let mut offset = 0usize;
    loop {
        let projects = selected_projects(&store, &filter)?;
        if !projects.is_empty() {
            for step in 0..projects.len() {
                let project = &projects[(offset + step) % projects.len()];
                if let Err(error) = daemon(&store, project, &credentials, &settings).tick() {
                    if error.is_lock_contention() {
                        eprintln!("depotd: {error}; continuing");
                    } else {
                        eprintln!("depotd: {}: {error}", project.slug);
                    }
                }
            }
            offset = (offset + 1) % projects.len();
        }
        thread::sleep(home.load_settings()?.poll_interval());
    }
}

fn daemon<'a>(
    store: &'a Store,
    project: &'a Project,
    credentials: &'a Credentials,
    settings: &'a Settings,
) -> Daemon<'a, Boxr, Treehouse, ShellValidation, ForgeDelivery<GitHub>, Box<dyn EventHook>> {
    let config = store.project_config(project).expect("project config");
    let hook: Box<dyn EventHook> = match &settings.on_event {
        Some(on_event) => Box::new(ShellEventHook::new(on_event.command.clone())),
        None => Box::new(NoEventHook),
    };
    Daemon::new(
        store,
        project.clone(),
        Boxr::new(Program::new("boxr")),
        Treehouse::new(Program::new("treehouse")),
        ShellValidation,
        ForgeDelivery::new(
            GitHub::new(DEFAULT_API_BASE, credentials.token.clone()),
            config.pull_request.base,
        ),
        hook,
    )
}

fn selected_projects(store: &Store, filter: &Option<String>) -> depotd::Result<Vec<Project>> {
    match filter {
        Some(name) => Ok(vec![select_project(store, Some(name))?]),
        None => store.projects(),
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
