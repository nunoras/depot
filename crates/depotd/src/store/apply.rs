use depot_core::{Action, Fact};

use crate::checklist::render_checklist;
use crate::error::Result;
use crate::project::Project;
use crate::store::coordinators::{clear_session, write_session};
use crate::store::tasks::write_task;
use crate::store::{EventOutcome, Store, with_lock_retry, write_event};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub outcome: EventOutcome,
    pub actions: Vec<Action>,
}

impl Store {
    pub fn apply_fact(&self, project: &Project, key: &str, fact: &Fact) -> Result<Applied> {
        self.apply_facts(project, &[(key.to_owned(), fact.clone())])
    }

    pub fn apply_facts(&self, project: &Project, facts: &[(String, Fact)]) -> Result<Applied> {
        with_lock_retry(|| self.apply_facts_once(project, facts))
    }

    fn apply_facts_once(&self, project: &Project, facts: &[(String, Fact)]) -> Result<Applied> {
        let transaction = self.connection().unchecked_transaction()?;
        let state = self.project_state(project)?;
        let mut next = state.clone();
        let mut actions = Vec::new();
        for (key, fact) in facts {
            if write_event(&transaction, &project.id, key, fact)? == 0 {
                transaction.rollback()?;
                return Ok(Applied {
                    outcome: EventOutcome::Duplicate,
                    actions: Vec::new(),
                });
            }
            let (reduced, intended) = depot_core::reduce(&next, fact);
            next = reduced;
            actions.extend(intended);
        }
        let project_home = self.home().project_home(&project.slug);
        project_home.ensure()?;
        for (id, task) in &next.tasks {
            if state.tasks.get(id) != Some(task) {
                write_task(&transaction, task)?;
            }
        }
        if next.coordinator != state.coordinator {
            match &next.coordinator {
                Some(session) => write_session(&transaction, &project.id, session)?,
                None => clear_session(&transaction, &project.id)?,
            }
        }
        std::fs::write(
            project_home.checklist_path(),
            render_checklist(&next, false),
        )?;
        transaction.commit()?;
        Ok(Applied {
            outcome: EventOutcome::Recorded,
            actions,
        })
    }
}
