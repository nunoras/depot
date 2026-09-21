# depot

[![CI](https://github.com/nunoras/depot/actions/workflows/ci.yml/badge.svg)](https://github.com/nunoras/depot/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

A local daemon that owns a project's agent work as deterministic facts. A model annotates the work. It never transitions a task.

depot records tasks, dependency edges, worker sessions, validation results and forge state in structured records. It renders a live checklist from those records, launches workers into pooled isolated worktrees, runs your project's validation command against an exact commit, and opens a pull request once validation passes. Nothing about that state depends on a model's prose.

## depot keeps models out of the state machine

The lifecycle is a pure `reduce` function: give it a project state and a fact, get the next state and the actions to take. `depot-core` has zero dependencies - no filesystem, no network, no clock. Anything time-shaped arrives as a field on the fact. The same facts always produce the same state. That is what makes the lifecycle table-testable and what keeps a model's judgment from silently altering task state.

## depot splits coordinator and worker

One coordinator session per project is where you talk about the work. The coordinator files tasks, answers worker questions, and writes narrative documents. It never edits project code.

Workers are headless sessions with a narrow contract: change code, ask a question, or submit. The daemon launches them, not the coordinator. Each worker runs in a pooled worktree leased for its task.

## depot validates before it ships

When a worker submits, depot runs the project's validation command against the exact commit on that worktree. A pass is bound to that commit. A moved branch invalidates it. Only after validation passes does depot push the branch and open a pull request. With `merge = "after_checks"`, the daemon merges the PR itself once forge checks pass and no dependency pin is stale. `merge = "after_review"` additionally files a review-role task against the PR head and holds the merge until that review validates.

## depot manages dependencies

A task can depend on another task at a specific commit. The dependency pin is what a dependent started against, so it is also what can go stale. The lifecycle tracks this, and a merge of a different revision holds the task for a person instead of landing it.

## Get started

1. Install the dependencies depot shells out to:

- [boxr](https://github.com/nunoras/boxr) - launches agent sessions
- [treehouse](https://github.com/kunchenguid/treehouse) - pools reusable worktrees
- [GitHub CLI](https://cli.github.com/) - forge auth (`gh auth login`)

```
# boxr
cargo install --git https://github.com/nunoras/boxr --locked

# treehouse (macOS / Linux)
curl -fsSL https://kunchenguid.github.io/treehouse/install.sh | sh

gh auth status
```

2. Install depot (Rust 1.98 or newer):

```
git clone https://github.com/nunoras/depot
cd depot
cargo install --locked --path crates/depot
cargo install --locked --path crates/depotd
```

Or from the repository without a prior clone:

```
cargo install --git https://github.com/nunoras/depot --locked --path crates/depot
cargo install --git https://github.com/nunoras/depot --locked --path crates/depotd
```

Confirm everything answers:

```
depot --help
depotd --help
boxr --version
treehouse status
gh auth status
```

3. Set up the depot home (`~/.depot/config.toml`):

```toml
concurrency = 1
poll_interval_seconds = 10
pool_root = "/home/you/.treehouse"

[credentials]
github = "gh-cli"

[profiles."my-builder"]
harness = "pi"
model = "xai/grok-4.5"
effort = "high"
account = ""
```

4. Register a project:

```
depot project add /path/to/your-project
```

This creates the project home under `~/.depot/projects/<slug>/` and adds `.depot.toml` to the repo's `.git/info/exclude` so it stays machine-local.

5. Configure the project (`.depot.toml` in the repo root; machine-local, never committed):

```toml
base_branch = "main"
max_concurrent_tasks = 1

[profiles]
build = "my-builder"

[validation]
command = "cargo test"

[pull_request]
base = "main"
merge = "manual"
```

6. Start the daemon:

```
depotd
```

7. File a task through the coordinator or directly:

```
depot task add --title "add rate limiting" --intent "add request rate limiting to the API" --role build
```

8. Approve it:

```
depot task approve <task-id>
```

The daemon leases a worktree, launches a boxr session, validates, pushes the branch and opens a PR.

## How it works

depot is three Rust crates:

| crate | role |
|---|---|
| `depot-core` | Pure domain model and lifecycle. Zero dependencies. |
| `depotd` | Daemon: store, adapters, configuration, coordinator artifacts. |
| `depot` | CLI that the coordinator, workers and users drive. |

The daemon talks to five things, each behind a thin adapter:

| boundary | what it talks to |
|---|---|
| sessions | boxr, for launching and observing agent sessions |
| worktrees | treehouse, for pooled worktree management |
| forge | the GitHub API, for PRs and merge observation |
| profiles | the project's role-to-profile map |
| dispatch | the Typesafe Choice API, for model-matched rule resolution |

Everything depot owns lives in the depot home (`$DEPOT_HOME`, default `~/.depot`), including a SQLite database that holds projects, tasks, dependency edges, attempts, validation records, observed forge state and the event journal. Project data and machine-local settings never mix: `.depot.toml` stays out of version control, and `config.toml` stays in the depot home.

### Task lifecycle

A task moves through: proposed, approved, queued (waiting for a slot), running, validating, pull-request-open and landed. A task that stops making progress ends as failed. Cancellation is explicit. The daemon reconciles records every tick: it leases worktrees, launches sessions, resumes answered questions, validates submitted commits, pushes branches, opens PRs and observes merges. Each pass derives from the stored records rather than from the actions a previous tick produced.

### Fair scheduling

Worker slots are capped store-wide by `concurrency`. Each project sets `max_concurrent_tasks` to bound how many slots it may hold. The daemon hands out work round-robin across projects so a busy project cannot starve the rest.

### Notifications

Set an `[on_event]` block in `config.toml` to run a command when progress blocks on a person:

```toml
[on_event]
command = "curl -sf -d @- https://ntfy.sh/my-depot"
events = ["question", "failed", "merge_refused"]
```

### Dispatch rules

When a task has no explicit `--role`, depot asks the Typesafe Choice API which of the project's dispatch rules matches the task title. A matching rule supplies the role and optionally pins a worker profile. `--role` is always the override.

## Examples

Check what needs attention:

```
depot status
depot inbox
```

File a task without naming a role (dispatch rules resolve it):

```
depot task add --title "fix the date parser" --intent "the ISO parser rejects leap seconds"
```

Stop a running task:

```
depot task stop t-3
```

Retry a failed task:

```
depot task retry t-3
```

Write a context document the coordinator session can read on rotation:

```
depot doc write context --content "this project uses..."
```

Run the daemon for one project only:

```
depotd --project my-project
```

## Limitations

- depot requires [boxr](https://github.com/nunoras/boxr) for session launches and [treehouse](https://github.com/kunchenguid/treehouse) for worktree pooling. Both must be installed and on `PATH`.
- Only GitHub is supported as a forge. Other forges have no adapter.
- Dispatch rules require a Typesafe API key. Without one, every task needs an explicit `--role`.
- The SQLite store does not replicate. depot is a single-machine tool.
- Rebase on conflict is limited to one attempt per task and requires a fix profile.
- There is no web dashboard. The checklist is a rendered Markdown file in the project home.
- Automatic merge is opt-in: `merge = "manual"` (default), `"after_checks"` or `"after_review"`. `pull_request.auto_merge` is deprecated and maps to `"after_checks"` when true.
- depot is in active development. The store schema migrates forward but the CLI surface may change.

## References

- [boxr](https://github.com/nunoras/boxr) - the session launcher depot drives.
- [treehouse](https://github.com/kunchenguid/treehouse) - the worktree pool depot leases from.
- [Context and glossary](CONTEXT.md) - the domain language depot uses.
- [Architecture decisions](docs/adr/) - recorded decisions that shape the code.
- [Daily driver guide](docs/daily-driver.md) - how to open depot as the main agent coordinator for a real project.
- [boxr contract](docs/boxr-contract.md) - what depot requires of boxr, command by command.
- [First real project report](docs/first-real-project-report.md) - what the first end-to-end run proved.

## License

Apache-2.0. See [LICENSE](LICENSE).

## Status

depot is in active development. It runs real projects on a single machine today. The store schema migrates forward automatically, but the CLI surface and configuration format may change between versions.
