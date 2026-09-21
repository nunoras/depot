# depot verification map

This directory is the maintained source for verifying what a depot user touches.
Read this index, then use the matching feature file as the recipe.

## Baseline preconditions

- Run from the repo root of a depot checkout that builds with `cargo build`.
- Run `.claude/skills/verify-depot/scripts/verify-depot.sh doctor` first.
  It must print `result: ok`.
- The shell must not set `DEPOT_HOME`, `DEPOT_TASK_ID` or `DEPOT_ATTEMPT_ID`.
- The driver owns the throwaway state: `DEPOT_HOME`, `BOXR_HOME`, `TREEHOUSE_ROOT` and `GH_CONFIG_DIR` all point into `${TMPDIR:-/tmp}/depot-verify.XXXXXX`.
- The fixture is `<throwaway>/verify-demo` (slug `verify-demo`) with `origin` at `<throwaway>/remote.git`, one profile `verify-haiku`, and `build` and `fix` mapped to it.
- Never drive a depot home or a `depotd` this run did not create.

## Driving conventions

- Every feature starts from the fixture.
  Only the feature's own commands change state.
- Run `depot` from the fixture repo unless a step names the store or the throwaway root.
- Treat every command as literal.
  Task ids are `t-1`, `t-2` in the order they are added.
- Never approve a task while `depotd` runs.
  That launches a real paid worker.
- To drive by hand, reproduce the exports at the top of `../scripts/verify-depot.sh` in a fresh shell first.

## Proof and skip reporting

- CLI proof is the command, stdout, stderr and exit code, all in `transcript.txt`.
- State proof is a second view: the checklist, a `depot status` call, the journal in `events.txt`, or the files on disk.
- Daemon proof also shows what did not happen: no worktree lease, no boxr session, held tasks unchanged.
- Report a path the driver cannot reach with the command you tried and the precondition it needs.
- Never report a skipped entry point as verified through another one.

## Feature entry contract

Each feature file starts with an H1 and one paragraph on the user-visible behavior, then exactly four H2 sections in this order.

1. `Sub-features` lists short ids with one line each.
2. `How to get to it (user POV)` lists every entry point a user has.
3. `Driving it with verify-depot.sh` starts with `Preconditions:`, then labeled bullets pairing each action with the exact command and the observable result.
4. `Gotchas` lists what wastes a run, leaves traces, or fails misleadingly.

A feature file is also a claim that the feature exists today.
A change that adds user-facing surface adds or extends its feature file and wires it into `../scripts/verify-depot.sh`.

## Features

- [project-add](./project-add.md) covers registration, the store scaffold, the machine-local config ignore, re-add and the profile refusal.
- [task-lifecycle](./task-lifecycle.md) covers filing, the dispatch refusal, approve, stop, acknowledge, retry, the inbox and the worker guard.
- [status-tui](./status-tui.md) covers the plain checklist, history and the live TUI.
- [doc-write](./doc-write.md) covers coordinator documents in the project store.
- [daemon](./daemon.md) covers `depotd` startup, its lock, heartbeat and restart recovery with only held work.
- [worker-loop](./worker-loop.md) covers the paid worker loop.
  It is manual and gated, and the driver does not run it.
