# The boxr surface depot requires

Depot never launches a harness itself.
boxr is the launcher and the ledger: it starts a headless session, and depot reads the session's state from boxr's own commands.
This file records the exact commands and output keys depot reads, and the minimum boxr version it accepts, so boxr's build of detached sessions ([nunoras/boxr#6](https://github.com/nunoras/boxr/issues/6)) and resume ([nunoras/boxr#7](https://github.com/nunoras/boxr/issues/7)) meet the surface depot now calls.
The reader is `crates/depotd/src/adapters/sessions.rs`; the contract test that drives it is `crates/depotd/tests/sessions.rs`.

## Minimum version

Depot requires boxr **0.2.0 or newer**, the first release that can carry #6 and #7.
boxr 0.1.0 ships none of the surface below, so depot refuses it.

The version is a floor, not the gate.
At daemon startup depot runs the capability probe and refuses any boxr whose `--help` does not name every command and flag in the table, listing the ones that are missing.
A missing capability is a loud failure naming the command, never a silent degradation.
Bump `MINIMUM_BOXR_VERSION` in `crates/depotd/src/adapters/sessions.rs` when boxr ships the surface under a different version number.

## The commands depot calls

Every command below is run as recorded, with the lease's worktree as the working directory for a launch.
`<id>` is the session id boxr printed at launch.

| depot reads | command | exit |
|---|---|---|
| the version | `boxr --version` | 0 |
| the capability surface | `boxr --help` | 0 |
| launch a session | `boxr --harness <harness> --model <model> --effort <effort> --account <profile> --kind <kind> --detach "<prompt>"` | 0 |
| resume a session | `boxr resume <id> "<prompt>"` | 0 |
| one session's state | `boxr status <id>` | 0 |
| turn end | `boxr wait <id>` or `boxr wait <id> --timeout <seconds>` | 0 |
| stop a session | `boxr stop <id>` | 0 |
| list sessions | `boxr ps` | 0 |

`--kind` is passed only when the task's role resolves to one of boxr's kinds, and the prompt is always the last argument.
`boxr tail` is not part of this surface: depot observes turn end through `boxr wait`, never by reading the harness's own output or boxr's ledger files.

## The output depot reads

boxr prints axi-style TOON.
Depot reads a strict subset: `key: value` fields, `key: "quoted value"` scalars, string arrays (`key[n]:` with one row per line), and tabular arrays (`key[n]{a,b,c}:` with comma-separated rows, quoted fields allowed).
Rows are indented under their header, sections (`key:`) qualify the keys below them, and a header whose announced row count does not match its rows is refused.
Depot never reads boxr's ledger on disk; the CLI is the interface.

| command | what depot requires on stdout |
|---|---|
| launch | `session: <id>`, or an `id: <id>` field, or a bare session id on stdout |
| `status` | `state: running\|finished\|stopped\|interrupted\|failed` |
| `wait` | `status: ok\|failed\|interrupted\|running` |
| `ps` | `sessions[N]{id,state,harness,model}:` with one row per session |

`status: ok` means the turn ended and the harness exited zero, `failed` means the turn ended with a harness failure, `interrupted` means the session was stopped from outside, and `running` means the wait returned before the turn ended.

A state or status value outside those sets is a loud failure naming the value, because depot will not translate a session state it does not know into a task state.

## What depot does with a failure

Any non-zero exit, from any of these commands, is reported with the boxr command line depot issued and boxr's stderr, so a capability that is missing at runtime is visible rather than absorbed.

## One difference from boxr#6's text

boxr#6 says `boxr wait --timeout` "exits non-zero without killing the session" when the timeout expires.
Depot reads an expired timeout from `status: running` with exit 0, and treats a non-zero exit from `wait` as a failure, because a non-zero exit cannot be told apart from a real error.
boxr and depot must agree on one of the two before the wiring ticket lands.
