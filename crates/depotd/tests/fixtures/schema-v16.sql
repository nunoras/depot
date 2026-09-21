
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


CREATE INDEX tasks_by_project_state ON tasks (project_id, state);
CREATE INDEX events_by_project ON events (project_id, id);


CREATE TABLE events_v3 (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    key        TEXT NOT NULL,
    at         INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    UNIQUE (project_id, key)
);
INSERT INTO events_v3 (id, project_id, key, at, kind, payload)
    SELECT id, project_id, key, at, kind, payload FROM events;
DROP TABLE events;
ALTER TABLE events_v3 RENAME TO events;
CREATE INDEX events_by_project ON events (project_id, id);


ALTER TABLE events ADD COLUMN task_id TEXT;

CREATE TABLE coordinators (
    project_id     TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    session        TEXT,
    started_at     INTEGER,
    context_tokens INTEGER,
    inbox_cursor   INTEGER NOT NULL DEFAULT 0
);


ALTER TABLE tasks ADD COLUMN submission_summary TEXT;
CREATE TABLE task_submission_artifacts (
    project_id TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    position   INTEGER NOT NULL,
    path       TEXT NOT NULL,
    PRIMARY KEY (project_id, task_id, position),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id) ON DELETE CASCADE
);

ALTER TABLE tasks ADD COLUMN dispatch_profile TEXT;
ALTER TABLE tasks ADD COLUMN merge_refused TEXT;
ALTER TABLE tasks ADD COLUMN acknowledged_at INTEGER;
ALTER TABLE tasks ADD COLUMN hold_pr INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_attempts ADD COLUMN rebase INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_attempts ADD COLUMN last_seen_at INTEGER;
ALTER TABLE tasks ADD COLUMN redirect_text TEXT;
ALTER TABLE tasks ADD COLUMN redirect_delivered INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN rework_of TEXT;
CREATE TABLE task_counters (
    project_id   TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    next_number  INTEGER NOT NULL
);
ALTER TABLE task_validations ADD COLUMN base_commit TEXT;
PRAGMA user_version = 16;
