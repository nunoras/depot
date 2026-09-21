pub mod adapters;

mod artifacts;
mod build_info;
mod checklist;
mod clock;
mod commands;
mod config;
mod coordinator;
mod daemon;
pub mod describe;
mod dispatch;
mod documents;
mod error;
pub mod evidence;
mod factcodec;
mod home;
mod inbox;
mod project;
mod projects;
mod restart;
mod settings;
mod store;
mod supervisor;
mod vocabulary;

pub use artifacts::{Staged, add_artifact, exactly_one_url};
pub use build_info::{BUILD_ID, VERSION, version_line};
pub use checklist::{format_timestamp, render_checklist, render_checklist_observed};
pub use commands::{
    MIN_WAIT_POLL, TaskRequest, Waited, acknowledge_task, add_task, answer_question, approve_tasks,
    ask_question, ensure_profiles_resolve, read_inbox, redirect_task, release_task, retry_task,
    rework_task, stop_task, submit_task, wait_for_task, write_narrative,
};
pub use config::{
    EvidenceConfig, MergePolicyConfig, PROJECT_CONFIG_FILE_NAME, ProjectConfig, PullRequestConfig,
    QuestionsConfig, ValidationConfig,
};
pub use coordinator::{
    BRIEF_TEMPLATE, COORDINATOR_KICKOFF_TEMPLATE, COORDINATOR_POLICY, CoordinatorContext, Launch,
    WorkerLaunch, render_template,
};
pub use daemon::{
    DAEMON_LOCK_FILE_NAME, Daemon, DaemonScope, Delivery, EventHook, EventNotice, ForgeDelivery,
    InstanceLock, NoEventHook, ObservedPullRequest, ShellEventHook, ShellValidation,
    ValidationResult, ValidationRunner, daemon_build_mismatch, daemon_scope, daemon_scope_covers,
    resume_prompt,
};
pub use depot_core::{ProjectState, Role, SessionId, Task, TaskId, TaskState, Timestamp};
pub use documents::{document_path, write_document};
pub use error::{Error, Result};
pub use evidence::{EvidenceArtifact, EvidenceRunner, ResolvedArtifact, ShellEvidence};
pub use home::{
    ARCHIVE_DIR_NAME, ARTIFACTS_DIR_NAME, CHECKLIST_FILE_NAME, CONTEXT_DOCUMENT_FILE_NAME,
    DATABASE_FILE_NAME, DOCUMENTS_DIR_NAME, DepotHome, HOME_ENV, MEDIA_DIR_NAME, PROJECTS_DIR_NAME,
    ProjectHome, SCRATCH_DIR_NAME, SETTINGS_FILE_NAME, slug_for,
};
pub use inbox::{InboxEntry, Need, inbox_entries, render_inbox};
pub use project::{LocationKind, Project};
pub use projects::{
    Added, StatusSelection, add_project, render_projects, render_status, render_status_at,
    select_project,
};
pub use restart::{
    DAEMON_LOG_FILE_NAME, DAEMON_STOP_FILE_NAME, LaunchSpec, RestartOptions, Restarted,
    clear_stop_request, installed_daemon, launch_detached, launch_spec, restart_daemon,
    stop_requested,
};
pub use settings::{
    ArtifactsSettings, DEFAULT_ON_EVENTS, OnEventSettings, ProfileSettings, Settings,
};
pub use store::{Applied, EventOutcome, RecordedEvent, SCHEMA_VERSION, Store, event_key};
pub use supervisor::Supervisor;
pub use vocabulary::{ROLE_NAMES, role_from_name, role_name, state_name};
