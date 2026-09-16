# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- The destination is the spec at https://github.com/nunoras/depot/issues/30, with the decisions behind it on the map at https://github.com/nunoras/depot/issues/29 and the tickets those two produced. Read the spec before changing behaviour; it owns the vocabulary, the state list and the v1 cut line.
- Rust workspace, one crate per layer: `crates/depot-core` (pure rules), `crates/depotd` (daemon), `crates/depot` (CLI). Gates: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- `depot-core` is the one testing seam for the lifecycle rules, and it takes facts in and returns state plus intended actions out. It must stay free of filesystem, network, processes and wall clock, so anything time-shaped arrives as a fact field. It declares no dependencies; `crates/depot-core/tests/purity.rs` guards that and `README.md` states it.
- `crates/depot-core/tests/lifecycle.rs` holds one table per lifecycle rule.
- `crates/depotd/src/adapters/` holds the four boundaries depot talks to (boxr sessions, treehouse worktrees, the forge, role profiles). Each is a thin trait plus an implementation that speaks the real protocol; adapters hold no policy, because the rules stay in `depot-core`. `crates/depotd/tests/` has one contract test per adapter, driving the real implementation against a fake that reproduces the protocol's success, failure and malformed cases. `docs/adr/0002-adapters-behind-seams.md` records why.
- What depot requires of boxr, command by command and field by field, is recorded in `docs/boxr-contract.md`, along with the minimum boxr version. boxr's detached sessions (nunoras/boxr#6) and resume (nunoras/boxr#7) are not built yet, so the capability probe in the session adapter is what refuses a boxr that cannot do the job, by name. Update that file and `MINIMUM_BOXR_VERSION` together when boxr lands.
- Every state transition is driven by a fact, never by model prose. `docs/adr/0001-local-daemon-and-deterministic-facts.md` records why, and `CONTEXT.md` is the glossary for the domain language.
- Persistence and configuration live in `crates/depotd`: `Store` is the sqlite record of projects, tasks, dependency edges, attempts, questions, validations, forge state and the event journal, `ProjectConfig` is the committed `.depot.toml` holding project knowledge, and `Settings` is the machine-local `<depot home>/config.toml`. The two halves of that split never mix; `crates/depotd/tests/config_split.rs` guards it and `README.md` documents the layout.
- `render_checklist` is a pure function of a `ProjectState`. Checklist bytes are written only by depotd write paths (`add_project` and `Store::put_task`), never by hand. Same state, same bytes; `crates/depotd/tests/checklist.rs` asserts it, so nothing time-shaped or order-shaped may enter the render.
- No comments in code, no docstrings, no TODOs. Rationale belongs in the commit message, the ticket, or an ADR.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
