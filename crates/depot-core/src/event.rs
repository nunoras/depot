use crate::model::{Task, TaskState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingEvent {
    Question,
    Failed,
    MergeRefused,
    Landed,
}

impl BlockingEvent {
    pub fn name(self) -> &'static str {
        match self {
            BlockingEvent::Question => "question",
            BlockingEvent::Failed => "failed",
            BlockingEvent::MergeRefused => "merge_refused",
            BlockingEvent::Landed => "landed",
        }
    }
}

pub fn blocking_event(task: &Task) -> Option<BlockingEvent> {
    match task.state {
        TaskState::WaitingOnQuestion if task.has_unanswered_question() => {
            Some(BlockingEvent::Question)
        }
        TaskState::Failed => Some(BlockingEvent::Failed),
        TaskState::PrOpen if task.merge_refused.is_some() => Some(BlockingEvent::MergeRefused),
        TaskState::Landed => Some(BlockingEvent::Landed),
        _ => None,
    }
}
