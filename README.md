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
| `crates/depotd` | The daemon: everything with a side effect, including the store, the depot home and configuration. |
| `crates/depot` | The command line the coordinator and the user drive. |

## The depot home

Everything depot owns lives outside your repositories, in one home: `$DEPOT_HOME`, or `~/.depot` when that variable is unset.

```
<depot home>
  config.toml          machine-local settings
  depot.db             one sqlite database: projects, tasks, dependency edges, attempts,
                       questions, validation records, observed forge state, event journal
  projects/<slug>/
    checklist.md       rendered from the records, never edited by hand
    archive/
    docs/
    scratch/
    media/
```

`depot project add <path-or-url>` registers a project, writes its home directory, and scaffolds its committed config.
Adding the same project twice is idempotent; a path that is not an existing directory is refused.
`depot status [--project <name>] [--all]` prints the rendered checklist.

The store applies its schema migrations on open and refuses a database written by a newer build rather than downgrading it.
`Store` is the only writer of `checklist.md`, and `render_checklist` is a pure function of a `ProjectState`: the same state always produces the same bytes.
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

[credentials]
github = "gh-cli"
```

Neither file accepts a key from the other side of the split, and registering a project never writes a machine-local setting into the repository.
`crates/depotd/tests/config_split.rs` is the guard.

## The pure core

`depot-core` is the one testing seam for the risky rules.
Its entry point is `reduce`, which takes a project state and a fact and returns the next state plus the actions the daemon should take.

```rust
let (state, actions) = reduce(&state, &fact);
```

`depot-core` has no dependencies at all, which is asserted by an empty `[dependencies]` table in `crates/depot-core/Cargo.toml` and guarded by `crates/depot-core/tests/purity.rs`.
No filesystem, no network, no processes and no wall clock: anything time-shaped arrives as a fact field, so the same facts always produce the same state.
That is what makes the lifecycle table-testable and what keeps a model's prose out of the state machine.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

`crates/depot-core/tests/lifecycle.rs` holds one table per lifecycle rule, each row a scenario asserting the resulting task state and the exact actions the daemon intends to take.
The rules are numbered in the ticket that built this skeleton: [nunoras/depot#31](https://github.com/nunoras/depot/issues/31).
`crates/depot-core/tests/purity.rs` keeps the core dependency-free, and `crates/depot/tests/cli.rs` drives the real binary.

## Where to read next

- `CONTEXT.md` is the glossary for the domain language.
- `docs/adr/` records the decisions that shape the code.
- `AGENTS.md` is the project's agent memory.
