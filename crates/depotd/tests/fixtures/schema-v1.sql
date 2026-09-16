CREATE TABLE projects (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE tasks (
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    id               TEXT NOT NULL,
    title            TEXT NOT NULL,
    intent           TEXT NOT NULL,
    role             TEXT NOT NULL,
    state            TEXT NOT NULL,
    base_dependency  TEXT,
    branch_head      TEXT,
    retry_profile    TEXT,
    retry_not_before INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE task_dependencies (
    project_id   TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    position     INTEGER NOT NULL,
    prerequisite TEXT NOT NULL,
    commit_id    TEXT NOT NULL,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE task_attempts (
    project_id  TEXT NOT NULL,
    task_id     TEXT NOT NULL,
    position    INTEGER NOT NULL,
    session     TEXT,
    profile     TEXT NOT NULL,
    worktree    TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    outcome     TEXT NOT NULL,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE task_questions (
    project_id  TEXT NOT NULL,
    task_id     TEXT NOT NULL,
    position    INTEGER NOT NULL,
    text        TEXT NOT NULL,
    asked_at    INTEGER NOT NULL,
    answer_text TEXT,
    answered_by TEXT,
    answered_at INTEGER,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE task_validations (
    project_id      TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    position        INTEGER NOT NULL,
    command         TEXT NOT NULL,
    commit_id       TEXT NOT NULL,
    exit_code       INTEGER NOT NULL,
    duration_millis INTEGER NOT NULL,
    output_tail     TEXT NOT NULL,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE task_artifacts (
    project_id TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    position   INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    path       TEXT NOT NULL,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE task_links (
    project_id TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    position   INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    number     INTEGER,
    url        TEXT NOT NULL,
    checks     TEXT,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

CREATE TABLE events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    key        TEXT NOT NULL UNIQUE,
    at         INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL
);

PRAGMA user_version = 1;
