# The boxr surface depot requires

Depot never launches a harness itself.
boxr is the launcher and the ledger: it starts a headless session, and depot reads the session's state from boxr's own commands.
This file records the exact commands and output keys depot reads, and the minimum boxr version it accepts.
Detached sessions ([nunoras/boxr#6](https://github.com/nunoras/boxr/issues/6)), resume ([nunoras/boxr#7](https://github.com/nunoras/boxr/issues/7)), and the pi exit-fact / resume surface from [nunoras/boxr#40](https://github.com/nunoras/boxr/pull/40) are part of boxr **0.2.0**.
The reader is `crates/depotd/src/adapters/sessions.rs`; the contract test that drives it is `crates/depotd/tests/sessions.rs`.

## Minimum version

Depot requires boxr **0.2.0 or newer**.
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
| launch a session | `boxr --harness <harness> --model <model> --effort <effort> [--account <profile>] --kind <kind> --detach "<prompt>"` | 0 |
| resume a session | `boxr resume <id> "<prompt>"` | 0 |
| one session's state | `boxr status <id>` | 0 |
| turn end | `boxr wait <id>` or `boxr wait <id> --timeout <seconds>` | 0 |
| stop a session | `boxr stop <id>` | 0 |
| list sessions | `boxr ps` | 0 |

`--kind` is the task's role name (`plan`, `build`, `review`, or `fix`), each of which is a boxr core kind.
`--account` is omitted when the machine-local profile leaves account empty; pi has no isolated account directory, so a pi profile launches without `--account` and uses the harness default credentials.
The prompt is always the last argument.
`boxr tail` is not part of this surface: depot observes turn end through `boxr wait`, never by reading the harness's own output or boxr's ledger files.

## The output depot reads

boxr prints axi-style TOON.
Depot reads a strict subset: `key: value` fields, `key: "quoted value"` scalars, string arrays (`key[n]:` with one row per line), and tabular arrays (`key[n]{a,b,c}:` with comma-separated rows, quoted fields allowed).
Rows are indented under their header, sections (`key:`) qualify the keys below them, and a header whose announced row count does not match its rows is refused.
Depot matches fields by leaf name, so nested boxr 0.2.0 output (`session:` / `id:`, `state:`, `status:`) is read the same way as a flat document.
Depot never reads boxr's ledger on disk; the CLI is the interface.

| command | what depot requires on stdout |
|---|---|
| launch | `session: <id>`, or an `id: <id>` field (including under a `session:` section), or a bare session id on stdout |
| `status` | `state: running\|finished\|stopped\|interrupted\|failed` |
| `wait` | `status: ok\|failed\|interrupted\|running` |
| `ps` | `sessions[N]{id,state,harness,model}:` with one row per session |

`status: ok` means the turn ended and the harness exited zero, `failed` means the turn ended with a harness failure, `interrupted` means the session was stopped from outside, and `running` means the wait returned before the turn ended.

A state or status value outside those sets is a loud failure naming the value, because depot will not translate a session state it does not know into a task state.

## Wait exit contract

`boxr wait` exits **0** whenever it can report one of those four `status` values, including `failed`, `interrupted`, and an expired `--timeout` that returns `status: running`.
A non-zero exit means boxr could not run the command at all (unknown session, unreadable records), and depot treats that as a command failure.
This is the settled boxr 0.2.0 / PR 40 contract; earlier issue text that had timeout exit non-zero is superseded.

## What depot does with a failure

Any non-zero exit, from any of these commands, is reported with the boxr command line depot issued and boxr's stderr, so a capability that is missing at runtime is visible rather than absorbed.
