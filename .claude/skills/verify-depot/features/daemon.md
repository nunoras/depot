# The daemon

`depotd` is the one process per depot home that drives registered projects: it refuses to start without a forge credential, takes an exclusive lock recording its pid, scope and heartbeat, refuses a second instance, journals a restart and runs recovery each time it starts, and ticks on the configured poll interval.

## Sub-features

- `no-credential` refuses to start when neither `gh` nor `<home>/secrets/github-token` has a token.
- `lock-record` takes `run/depotd.lock` and writes `run/depotd.scope.json` with the pid, the projects in scope and a heartbeat.
- `single-instance` refuses a second `depotd` on the same home while the first keeps running.
- `heartbeat` advances the scope record's heartbeat each tick.
- `restart-recovery` journals `daemon_restarted:<pid>:<millis>` on every start.
- `idle-held` leaves held tasks alone: no lease, no boxr session.
- `stop` exits on SIGTERM.

## How to get to it (user POV)

- Run `depotd` or `depotd --project <name>` and leave it running.
- Read `<home>/run/depotd.scope.json` or `depot status` to see whether a daemon drives a project.

## Driving it with verify-depot.sh

Preconditions:

- The fixture is registered and holds only held tasks.
  The driver checks `tasks.state` is `proposed` for every task and refuses otherwise.
- The throwaway `BOXR_HOME` and `TREEHOUSE_ROOT` are empty.

- **Run it.** Run `.claude/skills/verify-depot/scripts/verify-depot.sh daemon`.
- **Held work.** `depot task add --title "Stay held" --intent "..." --role build` files `t-1`.
- **No credential.** `depotd` in the repo, before any token file exists.
  Exit 1, stderr `no GitHub credential`.
- **Start.** The driver writes a placeholder `secrets/github-token` (mode 600) and starts `depotd` in the background, then waits until `run/depotd.scope.json` holds `"pid":<that pid>` and `"projects":["<repo>"]`.
- **Second instance.** `depotd` again.
  Exit 1, stderr `another depot daemon already holds <home>/run/depotd.lock`, and the first pid is still alive.
- **Status.** `depot status` still shows `## Held - awaiting approval (1)`.
- **Stop and restart.** The driver sends SIGTERM to the first pid, checks it is gone, that `depotd-1.log` has `daemon_restarted:<pid>:` and that the journal holds `polled` facts, then starts a second daemon and stops it the same way.
- **Nothing launched.** `events.txt` has exactly two `daemon_restarted` rows and no `worktree_acquire` or `worker_turn` rows, `task-states-before.txt` equals `task-states-after.txt`, and `BOXR_HOME` and `TREEHOUSE_ROOT` are still empty.
- **Heartbeat.** The two heartbeat reads 2.5 seconds apart, recorded in `meta.txt` as `heartbeats`, must differ.
- **Proof.** `depotd-1.log`, `depotd-2.log`, `depotd-lock-1.json`, `events.txt` and the task state files.

## Gotchas

- Never approve a task in this feature.
  An approved task makes the next tick lease a treehouse worktree and launch a real boxr worker.
- `depotd` resolves the GitHub token with `gh auth token` first.
  Without the isolated `GH_CONFIG_DIR` it would pick up the machine's real token.
- `depotd` probes `boxr --help` at startup, so a missing or old boxr fails startup, not a tick.
- `depotd` writes its facts as JSON lines to stdout.
  That is the log to read.
- Launch it as a direct child and use `$!`.
  Wrapping it in `setsid` forks, so `$!` is not the daemon's pid.
