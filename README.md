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
| `crates/depotd` | The daemon: the four adapters depot talks to, and everything else with a side effect. |
| `crates/depot` | The command line the coordinator and the user drive. |

`depot` is a shell until its ticket lands, and the daemon loop is not wired to the adapters yet: [nunoras/depot#34](https://github.com/nunoras/depot/issues/34) does that.

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

`crates/depotd/tests/` holds one contract test per adapter: `sessions.rs`, `worktrees.rs`, `forge.rs` and `profiles.rs`.
Each drives the real implementation against a fake of the dependency, covering success, failure and malformed output, and each asserts the exact command depot issued.
The fakes live in `crates/depotd/tests/support/`: a scripted program on disk for boxr, treehouse and `gh`, a local HTTP endpoint for GitHub, and a real git repository with a real remote for the worktree safety check.
`crates/depotd/tests/toon.rs` covers the TOON reader the session adapter parses boxr's output with.

## Where to read next

- `CONTEXT.md` is the glossary for the domain language.
- `docs/adr/` records the decisions that shape the code.
- `AGENTS.md` is the project's agent memory.
