use crate::daemon::Daemon;
use crate::error::Result;
use crate::project::Project;
use crate::store::Store;

pub struct Supervisor<'a, S, W, V, D, H> {
    store: &'a Store,
    projects: Vec<Project>,
    sessions: S,
    worktrees: W,
    validation: V,
    delivery: D,
    hook: H,
}

impl<'a, S, W, V, D, H> Supervisor<'a, S, W, V, D, H> {
    pub fn new(
        store: &'a Store,
        projects: Vec<Project>,
        sessions: S,
        worktrees: W,
        validation: V,
        delivery: D,
        hook: H,
    ) -> Self {
        Self {
            store,
            projects,
            sessions,
            worktrees,
            validation,
            delivery,
            hook,
        }
    }
}

impl<'a, S, W, V, D, H> Supervisor<'a, S, W, V, D, H>
where
    S: Clone + crate::adapters::sessions::Sessions,
    W: Clone + crate::adapters::worktrees::Worktrees,
    V: Clone + crate::daemon::ValidationRunner,
    D: Clone + crate::daemon::Delivery,
    H: Clone + crate::daemon::EventHook,
{
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn recover(&self) -> Result<()> {
        for project in &self.projects {
            if let Err(error) = self.daemon(project).recover() {
                if error.is_project() {
                    crate::daemon::log_project_error(&project.slug, &error);
                    continue;
                }
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn tick(&self, turn: usize) -> Result<()> {
        let capacity = self.store.home().load_settings()?.concurrency;
        let mut budget = capacity.saturating_sub(self.in_flight()?);
        let count = self.projects.len();
        for offset in 0..count {
            let project = &self.projects[(turn + offset) % count];
            match self.daemon(project).tick_under(budget) {
                Ok(remaining) => budget = remaining,
                Err(error) if error.is_project() => {
                    crate::daemon::log_project_error(&project.slug, &error);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn in_flight(&self) -> Result<usize> {
        let mut total = 0;
        for project in &self.projects {
            total += self
                .store
                .tasks(&project.id)?
                .into_values()
                .filter(|task| {
                    task.state.in_flight()
                        && task
                            .attempts
                            .last()
                            .is_some_and(|attempt| attempt.worktree.is_some())
                })
                .count();
        }
        Ok(total)
    }

    fn daemon(&self, project: &Project) -> Daemon<'_, S, W, V, D, H> {
        Daemon::new(
            self.store,
            project.clone(),
            self.sessions.clone(),
            self.worktrees.clone(),
            self.validation.clone(),
            self.delivery.clone(),
            self.hook.clone(),
        )
    }
}
