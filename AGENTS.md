# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- The destination is the spec at https://github.com/nunoras/depot/issues/30, with the decisions behind it on the map at https://github.com/nunoras/depot/issues/29 and the tickets those two produced. Read the spec before changing behaviour; it owns the vocabulary, the state list and the v1 cut line.
- Rust workspace, one crate per layer: `crates/depot-core` (pure rules), `crates/depotd` (daemon), `crates/depot` (CLI). Gates: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- `depot-core` is the one testing seam for the lifecycle rules, and it takes facts in and returns state plus intended actions out. It must stay free of filesystem, network, processes and wall clock, so anything time-shaped arrives as a fact field. It declares no dependencies; `crates/depot-core/tests/purity.rs` guards that and `README.md` states it.
- `crates/depot-core/tests/lifecycle.rs` holds one table per lifecycle rule.
- Every state transition is driven by a fact, never by model prose. `docs/adr/0001-local-daemon-and-deterministic-facts.md` records why, and `CONTEXT.md` is the glossary for the domain language.
- No comments in code, no docstrings, no TODOs. Rationale belongs in the commit message, the ticket, or an ADR.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
