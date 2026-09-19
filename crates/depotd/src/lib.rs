pub mod adapters;

mod checklist;
mod clock;
mod commands;
mod config;
mod coordinator;
mod daemon;
mod dispatch;
mod documents;
mod error;
mod factcodec;
mod home;
mod inbox;
mod project;
mod projects;
mod settings;
mod store;
mod vocabulary;

pub use checklist::{format_timestamp, render_checklist};
pub use commands::{
    TaskRequest, acknowledge_task, add_task, answer_question, approve_tasks, ask_question,
    ensure_profiles_resolve, read_inbox, stop_task, submit_task, write_narrative,
};
pub use config::{
    PROJECT_CONFIG_FILE_NAME, ProjectConfig, PullRequestConfig, QuestionsConfig, ValidationConfig,
};
pub use coordinator::{
    BRIEF_TEMPLATE, COORDINATOR_KICKOFF_TEMPLATE, COORDINATOR_POLICY, CoordinatorContext, Launch,
    WorkerLaunch, render_template,
};
pub use daemon::{
    DAEMON_LOCK_FILE_NAME, Daemon, Delivery, EventHook, EventNotice, ForgeDelivery, InstanceLock,
    NoEventHook, ObservedPullRequest, ShellEventHook, ShellValidation, ValidationResult,
    ValidationRunner, pull_request_body, resume_prompt,
};
pub use depot_core::{ProjectState, TaskState};
pub use documents::{document_path, write_document};
pub use error::{Error, Result};
pub use home::{
    ARCHIVE_DIR_NAME, CHECKLIST_FILE_NAME, CONTEXT_DOCUMENT_FILE_NAME, DATABASE_FILE_NAME,
    DOCUMENTS_DIR_NAME, DepotHome, HOME_ENV, MEDIA_DIR_NAME, PROJECTS_DIR_NAME, ProjectHome,
    SCRATCH_DIR_NAME, SETTINGS_FILE_NAME, slug_for,
};
pub use inbox::{InboxEntry, Need, inbox_entries, render_inbox};
pub use project::{LocationKind, Project};
pub use projects::{Added, StatusSelection, add_project, render_status, select_project};
pub use settings::{DEFAULT_ON_EVENTS, OnEventSettings, ProfileSettings, Settings};
pub use store::{Applied, EventOutcome, RecordedEvent, SCHEMA_VERSION, Store, event_key};
pub use vocabulary::{ROLE_NAMES, role_from_name, role_name, state_name};
