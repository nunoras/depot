# depot

depot is a local daemon that owns the state of a project's agent work as deterministic facts.

It records tasks, dependencies, worker sessions, validation results and forge state in structured records, renders a live checklist from those records, launches workers into pooled isolated worktrees, runs a project's validation command against an exact commit, and opens a pull request once validation passes.
Nothing about that state depends on a model's prose.
A model may annotate the work; it never transitions a task.

The destination is the published spec at [nunoras/depot#30](https://github.com/nunoras/depot/issues/30), and the decisions behind it are on the map at [nunoras/depot#29](https://github.com/nunoras/depot/issues/29).

## Crates

| crate | what it holds |
|---|---|
| `crates/depot-core` | The domain model and the whole task lifecycle as one pure reduce step. |
| `crates/depotd` | The daemon: everything with a side effect, including the five adapters depot talks to, the store, the depot home, configuration, and the coordinator's artifacts. |
| `crates/depot` | The command line the coordinator and the user drive. |



## The depot home

Everything depot owns lives outside your repositories, in one home: `$DEPOT_HOME`, or `~/.depot` when that variable is unset.

```
<depot home>
  config.toml          machine-local settings
  depot.db             one sqlite database: projects, tasks, dependency edges, attempts,
                       questions, validation records, observed forge state, the coordinator's
                       session and inbox position, event journal
  projects/<slug>/
    checklist.md       rendered from the records, never edited by hand
    archive/
    docs/              narrative documents, including context.md
    scratch/
    media/
```

`depot project add <path-or-url>` registers a project and writes its home directory.
A path project also scaffolds its committed `.depot.toml` when that file is missing.
Adding the same project twice is idempotent; a path that is not an existing directory is refused.

`depot status` prints the rendered checklist for the project this directory belongs to.
`--project <name>` names one registered project, and `--all` lists every project.
A directory that matches none is an error that names the registered projects, and a directory inside a project's store belongs to that project.
A failed or cancelled task fades from the default view once a live task depends on it or a person acknowledges it with `depot task acknowledge <task-id>`; `--history` shows the faded tasks again.

The store applies its schema migrations on open and refuses a database written by a newer build rather than downgrading it.
`checklist.md` is rewritten from the records on registration and on every task write; hand edits do not stick.
`render_checklist` is a pure function of a `ProjectState`: the same state always produces the same bytes.
`crates/depotd/tests/` asserts that rather than assuming it.

## The configuration split

Project knowledge is committed with the repository, so validation commands and profiles are reviewed like code.

`.depot.toml`, in the project repository:

```toml
base_branch = "main"

[profiles]
build = "glm-5.3"

[validation]
command = "cargo test"

[pull_request]
base = "main"
auto_merge = false

[questions]
always_relay = false

[dispatch]
confidence_floor = 0.8

[[dispatch.rules]]
when = "Implement or repair software"
role = "build"
candidates = ["glm-5.3", "gpt-5.5"]

[[dispatch.rules]]
when = "Review a change"
role = "review"
```

A rule's `candidates` is an optional ordered list of profiles, and only its first entry is read today: it is the profile the matching task is pinned to.
The rest of the list is carried for the ticket that ranks candidates by quota, which is the ticket that will choose among them, so the order is not a fallback chain.
Failover is unchanged: it still comes from the machine-local `fallback_profiles` in the depot home, the same list the retry path already used.
A candidate that names no machine-local profile refuses task creation by name rather than failing at launch.

Machine-local settings stay in the depot home and never travel to another host.

`$DEPOT_HOME/config.toml`:

```toml
concurrency = 4
run_duration_minutes = 60
poll_interval_seconds = 30
pool_root = "/home/me/.treehouse"
fallback_profiles = ["gpt-5.5"]
coordinator_context_tokens = 120000
typesafe_base_url = "https://api.typesafe.ai"

[credentials]
github = "gh-cli"

[profiles."glm-5.3"]
harness = "pi"
model = "glm-5.3"
effort = "high"
account = ""

[profiles."gpt-5.5"]
harness = "claude"
model = "opus"
effort = "high"
account = "work"
```

`account` may be empty: depot then omits `--account` on the boxr launch.
pi has no isolated account directory, so a pi profile leaves it blank and uses the host credentials.

### On-event hook

When `[on_event]` is set in the depot home's `config.toml`, depotd runs the command with one JSON event on stdin each time progress blocks on the human.
The command owns delivery and retries; depotd never does HTTP itself.

```toml
[on_event]
command = "curl -sf -d @- https://ntfy.sh/my-depot"
events = ["question", "failed", "merge_refused"]
```

Without an `events` list the hook fires on the blocking events by default: `question` when a task waits on the user, `failed` when a task stops making progress, and `merge_refused` when an automatic merge was refused and the task is held.
`landed` is opt-in and fires when a pull request merges.
An event fires at most once per state change, and no `on_event` section means no hook and no change in behaviour.

Each event carries the project slug, the task id, the task title, the event name, the question and a recommended default when one exists, and the pull request link when one exists:

```json
{"project":"depot","task":"t-7","title":"on_event hook","event":"question","question":"Which store?","recommended_default":null,"pull_request":null}
```

Neither file accepts a key from the other side of the split, and registering a project never writes a machine-local setting into the repository.
`crates/depotd/tests/config_split.rs` is the guard.

### Dispatch rules

`depot task add` no longer requires `--role`.
Without one, depot asks Typesafe which of the project's `dispatch.rules` matches the task, and the matching rule supplies the role.
A rule's `when` is the natural-language condition it matches, `role` is the role a matching task gets, and `candidates` is an optional ordered list of profiles; nothing but the `when` strings reaches the model.
A rule with no candidates resolves its role through `[profiles]`.

`confidence_floor` defaults to `0.8` when a project sets nothing, and an answer below it refuses task creation rather than guessing a profile.
A task that matches no rule, an absent or malformed rules table, a missing Typesafe key and an API error each refuse with their own reason, and `--role` is always the override.
Without a `typesafe-key` file in the depot home the layer is off, no request is made, and `--role` behaves as it always has.
The decision is recorded as `task_dispatch_judged` before the task's proposal fact, marked `model_judgement`, and carrying the chosen rule, the confidence, the model id and version and a hash of the rule set.
`docs/adr/0004-model-matched-rules-resolve-in-code.md` records why the model picks the rule and never the profile.

## The coordinator contract

One coordinator session per project is where you talk about the work, and it is whatever boxr launches with the project's profile.
Nothing depot ships hardcodes a harness or a model id, and `crates/depotd/tests/coordinator.rs` asserts that.

The texts a coordinator is given are versioned artifacts in this repository, included in the binary:

| artifact | what it holds |
|---|---|
| `assets/coordinator-policy.md` | what the coordinator owns: no project code, when to delegate, when to answer a worker and when to relay, the checklist against narrative documents, and that new scope waits for the user. |
| `assets/coordinator-kickoff.md` | the first message of a session: the project, the store, the live checklist and the context document, then `depot inbox`. |
| `assets/brief-template.md` | the brief a worker is handed: intent, role, output destination, done criteria, dependencies, validation, worker context environment variables, the store, and the two worker calls. |

`depot inbox` prints the facts recorded since the coordinator's previous turn, joined to where each task stands now, split into what needs the user, what needs the coordinator, and what needs nothing.
A poll that observed nothing is not reported at all, and the read position lives on the project's coordinator row, so a rotated session picks up where the last one stopped.

A session is rotated past `coordinator_context_tokens`, the context size reported for it.
The rule lives in the pure core, the limit is machine-local configuration, and the session the daemon must stop comes from the state rather than from anyone's impression of how long the conversation feels.

The verbs a coordinator drives:

| verb | what it does |
|---|---|
| `depot task add --title <title> --intent <intent> [--role <role>] [--depends-on <task>@<commit>]... [--base-dependency <task-id>]` | records a task with its dependencies. Multiple dependencies need a base. It lands held, and that is the proposal. Without `--role` depot resolves the role from the project's dispatch rules. |
| `depot task approve <task-id>...` | the user's go, for one task or several in one message. |
| `depot task answer <task-id> --text <answer> [--by coordinator\|user]` | records an answer and resumes the worker. |
| `depot task stop <task-id>` | stops a task. |
| `depot task acknowledge <task-id>` | acknowledges a failed or cancelled task so it fades from the default status. |
| `depot status [--project <name>] [--all] [--history]` | the live checklist; `--history` shows faded tasks. |
| `depot doc write <name> --content <text\|->` | writes a narrative document under the project's `docs/`. |
| `depot inbox [--project <name>]` | what happened since the last turn. |

The two calls a worker uses to move state:

| verb | what it does |
|---|---|
| `depot ask --task <task-id> --project <name> --relay <question>` | records a relayed question, pauses the worker and waits for an answer from the coordinator or the user. |
| `depot submit --task <task-id> --project <name>` | records the submission, then triggers validation against HEAD. |

A worker with `DEPOT_TASK_ID` and `DEPOT_ATTEMPT_ID` set can only use `ask` and `submit`.
Every other command is refused to enforce the worker/coordinator boundary.
A worker may write narrative documents or artifacts: those change no task state.

A role with no entry in the project's `[profiles]` map is refused when the task is filed rather than defaulted to another profile.
`docs/context.md` is the coordinator's context document: depot scaffolds it on registration and never renders it, so a fresh session reads it to catch up and the session that wrote it stops mattering.

## Running the daemon

The daemon runs a continuous loop for one project, polling session status, launching workers, running validation, and delivering pull requests.

```sh
depotd --project <project>
```

The daemon acquires an exclusive lock on `$DEPOT_HOME/depotd.lock` to prevent multiple daemon instances.
Polling interval is controlled by the `poll_interval_seconds` setting in the depot home's `config.toml`.

Every tick reconciles the records before it acts: a task whose attempt holds no worktree is leased one, an attempt without a session is launched, an answer a worker has not been told about is resumed, a submitted commit is validated, a validated commit is published, and a task with an open pull request is observed at the forge.
Each pass is derived from the stored records rather than from the actions a fact produced, so a fact the coordinator's CLI wrote reaches its end without that process executing anything; `docs/adr/0003-reconciliation-derives-pending-work.md` records why.

`[pull_request] auto_merge` in `.depot.toml` is opt-in: with it set, the daemon merges a validated pull request itself once its checks pass and no dependency pin is stale, which lands the task, releases its worktree, deletes the delivery branch at the forge and drops it from the checklist.
A merge of any other revision is held for a person instead of landing, and a merge the forge refuses is named on the task and in `depot inbox`.
When a forge observation reports an open pull request as conflicting, the daemon schedules one rebase attempt on the same task with the fix role's profile: the rebase worker rebases the delivery branch onto the base branch and submits, and the usual validation, push and merge path lands it.
Only one rebase is in flight per project at a time, a task past its attempt limit gets no more, and a project without a fix profile keeps today's behavior.

On startup, the daemon performs recovery: tasks with an in-flight attempt are transitioned to `Unknown` state, allowing them to be restarted or reworked.

## The pure core

`depot-core` is the one testing seam for the risky rules.
Its entry point is `reduce`, which takes a project state and a fact and returns the next state plus the actions the daemon should take.

```rust
let (state, actions) = reduce(&state, &fact);
```

`depot-core` has no dependencies at all, which is asserted by an empty `[dependencies]` table in `crates/depot-core/Cargo.toml` and guarded by `crates/depot-core/tests/purity.rs`.
No filesystem, no network, no processes and no wall clock: anything time-shaped arrives as a fact field, so the same facts always produce the same state.
That is what makes the lifecycle table-testable and what keeps a model's prose out of the state machine.

## The adapters

Depot owns none of the five things it talks to, so each one sits behind a thin trait in `crates/depotd/src/adapters/`, with an implementation that speaks the real protocol at the edge.
An adapter holds no policy: the lifecycle rules stay in `depot-core`, and the adapter only turns a decision into a command and the answer back into a fact.

| boundary | adapter | the dependency |
|---|---|---|
| sessions | `sessions.rs` | `boxr`, the launcher and the ledger, driven as a child process |
| worktrees | `worktrees.rs` | `treehouse`, the worktree pool, plus git for the safety check |
| forge | `forge.rs` | the GitHub API over HTTP, with a configurable base URL |
| profiles | `profiles.rs` | the project's role to profile map |
| dispatch | `typesafe.rs` | the Typesafe Choice API over HTTP, with a configurable base URL and an owner-only key file |

What depot requires of boxr, command by command and field by field, is recorded in [`docs/boxr-contract.md`](docs/boxr-contract.md). boxr 0.2.0 ships the detached session, status, wait, ps and resume surface depot calls.
To open depot as the daily driver on a real project, see [`docs/daily-driver.md`](docs/daily-driver.md).
The first end-to-end run against boxr is summarized in [`docs/first-real-project-report.md`](docs/first-real-project-report.md).
A missing capability fails loudly, naming the command.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

`crates/depot-core/tests/lifecycle.rs` holds one table per lifecycle rule, each row a scenario asserting the resulting task state and the exact actions the daemon intends to take.
The rules are numbered in the ticket that built this skeleton: [nunoras/depot#31](https://github.com/nunoras/depot/issues/31).
`crates/depot-core/tests/purity.rs` keeps the core dependency-free, `crates/depotd/tests/` covers the store, home, config split and checklist, `crates/depotd/tests/coordinator.rs` covers the policy prompt, the kickoff, rotation and the brief, `crates/depotd/tests/inbox.rs` covers the payload from a seeded store, and `crates/depot/tests/cli.rs` drives the real binary.

`crates/depotd/tests/` holds one contract test per adapter: `sessions.rs`, `worktrees.rs`, `forge.rs`, `profiles.rs` and `typesafe.rs`.
Each drives the real implementation against a fake of the dependency, covering success, failure and malformed output, and each asserts the exact command depot issued.
The fakes live in `crates/depotd/tests/support/`: a scripted program on disk for boxr, treehouse and `gh`, a local HTTP endpoint for GitHub and for Typesafe, and a real git repository with a real remote for the worktree safety check.
`crates/depotd/tests/toon.rs` covers the TOON reader the session adapter parses boxr's output with.

`crates/depot/tests/golden_path.rs` is the end-to-end suite, and it is the check to run before believing depot works.
It drives the real `depot` binary against a real git repository with a real remote, a scripted worker, a fake boxr child process whose recorded invocations are asserted, a local fake forge endpoint and a local fake Typesafe endpoint, with the daemon loop run tick by tick over the same adapters the daemon binary builds.
Its fake boxr is the `fake_boxr` test target beside it, so a plain `cargo test` builds it before the suite runs.
Scenarios cover the whole journey, a worker question, a failed validation and a restart with a task in flight, plus the recovery edges each reconcile pass relies on, the forge half of the loop from the idle depot to the opted-in merge, and the dispatch path from a model-matched rule through to a pinned profile; each asserts the rendered checklist, the task's state history, the commands the daemon issued and the exit codes it saw.
`crates/depot/tests/support/` holds the fixture and includes the fakes under `crates/depotd/tests/support/` rather than duplicating them.
It reaches no external network, so it runs anywhere.

## Where to read next

- `CONTEXT.md` is the glossary for the domain language.
- `docs/adr/` records the decisions that shape the code.
- `AGENTS.md` is the project's agent memory.
