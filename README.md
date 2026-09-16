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
| `crates/depotd` | The daemon: everything with a side effect. |
| `crates/depot` | The command line the coordinator and the user drive. |

`depotd` and `depot` are shells until their tickets land.

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

## Where to read next

- `CONTEXT.md` is the glossary for the domain language.
- `docs/adr/` records the decisions that shape the code.
- `AGENTS.md` is the project's agent memory.
