use depot_core::{Action, Fact};

use crate::checklist::render_checklist;
use crate::error::Result;
use crate::project::Project;
use crate::store::coordinators::{clear_session, write_session};
use crate::store::tasks::write_task;
use crate::store::{EventOutcome, Store, write_event};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub outcome: EventOutcome,
    pub actions: Vec<Action>,
}

impl Store {
    pub fn apply_fact(&self, project: &Project, key: &str, fact: &Fact) -> Result<Applied> {
        let state = self.project_state(project)?;
        let (next, actions) = depot_core::reduce(&state, fact);

        let project_home = self.home().project_home(&project.slug);
        project_home.ensure()?;
        let checklist = render_checklist(&next);
        let checklist_path = project_home.checklist_path();

        let transaction = self.connection().unchecked_transaction()?;
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
        if write_event(&transaction, &project.id, key, fact)? == 0 {
            transaction.rollback()?;
            return Ok(Applied {
                outcome: EventOutcome::Duplicate,
                actions: Vec::new(),
            });
        }
        std::fs::write(&checklist_path, checklist)?;
        transaction.commit()?;
        Ok(Applied {
            outcome: EventOutcome::Recorded,
            actions,
        })
    }
}
