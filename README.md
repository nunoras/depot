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
| `crates/depotd` | The daemon: everything with a side effect, including the four adapters depot talks to, the store, the depot home, configuration, and the coordinator's artifacts. |
| `crates/depot` | The command line the coordinator and the user drive. |

The daemon loop is not wired to the adapters yet: [nunoras/depot#34](https://github.com/nunoras/depot/issues/34) does that.

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
```

Machine-local settings stay in the depot home and never travel to another host.

`$DEPOT_HOME/config.toml`:

```toml
concurrency = 4
run_duration_minutes = 60
poll_interval_seconds = 30
pool_root = "/home/me/.treehouse"
fallback_profiles = ["gpt-5.5"]
coordinator_context_tokens = 120000

[credentials]
github = "gh-cli"
```

Neither file accepts a key from the other side of the split, and registering a project never writes a machine-local setting into the repository.
`crates/depotd/tests/config_split.rs` is the guard.

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
| `depot task add --title <title> --intent <intent> --role <role> [--depends-on <task>@<commit>]... [--base-dependency <task-id>]` | records a task with its dependencies. Multiple dependencies need a base. It lands held, and that is the proposal. |
| `depot task approve <task-id>...` | the user's go, for one task or several in one message. |
| `depot task answer <task-id> --text <answer> [--by coordinator\|user]` | records an answer and resumes the worker. |
| `depot task stop <task-id>` | stops a task. |
| `depot status [--project <name>] [--all]` | the live checklist. |
| `depot doc write <name> --content <text\|->` | writes a narrative document under the project's `docs/`. |
| `depot inbox [--project <name>]` | what happened since the last turn. |

The two calls a worker uses to move state:

| verb | what it does |
|---|---|
| `depot ask <question>` | records a relayed question, pauses the worker and waits for an answer from the coordinator or the user. |
| `depot submit --summary <text> --artifact <path-or-url>...` | records the submission summary and artifacts, then triggers validation against HEAD. |

A worker with `DEPOT_TASK_ID` and `DEPOT_ATTEMPT_ID` set can only use `ask` and `submit`.
Every other command is refused to enforce the worker/coordinator boundary.
A worker may write narrative documents or artifacts: those change no task state.

A role with no entry in the project's `[profiles]` map is refused when the task is filed rather than defaulted to another profile.
`docs/context.md` is the coordinator's context document: depot scaffolds it on registration and never renders it, so a fresh session reads it to catch up and the session that wrote it stops mattering.

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

Depot owns none of the four things it talks to, so each one sits behind a thin trait in `crates/depotd/src/adapters/`, with an implementation that speaks the real protocol at the edge.
An adapter holds no policy: the lifecycle rules stay in `depot-core`, and the adapter only turns a decision into a command and the answer back into a fact.

| boundary | adapter | the dependency |
|---|---|---|
| sessions | `sessions.rs` | `boxr`, the launcher and the ledger, driven as a child process |
| worktrees | `worktrees.rs` | `treehouse`, the worktree pool, plus git for the safety check |
| forge | `forge.rs` | the GitHub API over HTTP, with a configurable base URL |
| profiles | `profiles.rs` | the project's role to profile map |

What depot requires of boxr, command by command and field by field, is recorded in [`docs/boxr-contract.md`](docs/boxr-contract.md), because detached sessions and resume are not built yet.
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

`crates/depotd/tests/` holds one contract test per adapter: `sessions.rs`, `worktrees.rs`, `forge.rs` and `profiles.rs`.
Each drives the real implementation against a fake of the dependency, covering success, failure and malformed output, and each asserts the exact command depot issued.
The fakes live in `crates/depotd/tests/support/`: a scripted program on disk for boxr, treehouse and `gh`, a local HTTP endpoint for GitHub, and a real git repository with a real remote for the worktree safety check.
`crates/depotd/tests/toon.rs` covers the TOON reader the session adapter parses boxr's output with.

## Where to read next

- `CONTEXT.md` is the glossary for the domain language.
- `docs/adr/` records the decisions that shape the code.
- `AGENTS.md` is the project's agent memory.
