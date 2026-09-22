---
name: verify-depot
description: Use when proving depot works the way a user drives it, through the real `depot` CLI and `depotd` daemon built from this checkout, instead of the fakes in `crates/depot/tests/golden_path.rs`. Covers `depot project add`, the task lifecycle (`task add --role`, approve, stop, acknowledge, retry, the Typesafe refusal, the worker-context guard), `depot status` and `depot inbox` including `status --tui`, `depot doc write`, and `depotd` start, instance lock, heartbeat and restart recovery. All of it runs in a throwaway agni home and spends no model quota. Run it after changing the CLI, the store, the checklist, the TUI or the daemon loop, and before a release.
---

# verify-depot

## What this proves

`golden_path.rs` drives the daemon tick by tick with a fake boxr, a fake forge and a fake Typesafe, so it proves the rules and never proves the shipped binaries behave for a user.
This skill builds `depot` and `depotd` from the checkout, runs them as a user would in a throwaway agni home against a throwaway git project, and saves every command, exit code, stdout and stderr as evidence.

## Isolation

Another depot may be live on this machine against `~/.depot` or `~/.agni`.
This skill never reads or writes it.

- Every run makes a throwaway directory `${TMPDIR:-/tmp}/depot-verify.XXXXXX` and points every home depot touches into it:
  - `AGNI_HOME=<throwaway>/home`, so `agni.db`, `config.toml`, `secrets/`, `run/` and the project stores stay inside.
  - `BOXR_HOME=<throwaway>/boxr`, so the real boxr ledger is never touched.
  - `TREEHOUSE_ROOT=<throwaway>/pool`, so a lease would land inside, not in `~/.treehouse`.
  - `GH_CONFIG_DIR=<throwaway>/gh`, with `GH_TOKEN` and `GITHUB_TOKEN` unset, so `gh auth token` finds nothing and `depotd` can never pick up the real GitHub token.
- The daemon gets a placeholder `secrets/github-token` file in the throwaway home, so any forge call it made would fail auth instead of writing to GitHub.
- No `typesafe-key` exists in the throwaway home, so dispatch without `--role` is refused before any network call.
- The fixture project is a local repo whose `origin` is a local bare repo in the throwaway.
  Nothing pushes to GitHub.
- The driver refuses to run when `AGNI_HOME` or `DEPOT_HOME` is already set, when `DEPOT_TASK_ID` or `DEPOT_ATTEMPT_ID` is set, or when the throwaway or the evidence root would land inside `~/.agni`, `~/.depot` or `~/.treehouse`.
- The daemon feature refuses to start `depotd` if any task is past held, because an approved task makes the daemon lease a worktree and launch a real paid worker.
- Processes are stopped by the pid the driver started, after checking `/proc/<pid>/cmdline` is this checkout's `depotd`.
  Nothing is killed by name.
- The TUI runs on a private tmux server (`tmux -L depot-verify-<pid>`), and cleanup kills only that server.

## Launch

There is no long-lived server to keep up.
Each run builds the binaries once, then starts each command, the daemon and the TUI as its own process:

```
cargo build --bin depot --bin depotd
```

The driver puts `target/debug` first on `PATH`, so `depot` is the build under test, never `~/.cargo/bin/depot`.
For the daemon, ready means `run/depotd.scope.json` in the throwaway home names the pid the driver started.

## The loop

Run the driver from the repo root:

```
.claude/skills/verify-depot/scripts/verify-depot.sh doctor
.claude/skills/verify-depot/scripts/verify-depot.sh <feature>
.claude/skills/verify-depot/scripts/verify-depot.sh cleanup <run-dir>
```

`<feature>` is one of `project-add`, `task-lifecycle`, `status-tui`, `doc-write`, `daemon`.
A feature run goes through five steps in order.

1. Guard.
   Makes the throwaway, exports the isolated homes, and refuses anything that would reach the real home.
2. Doctor.
   Builds both binaries, checks `git`, `sqlite3`, `boxr` (0.2.0 or newer), `treehouse`, `gh` and `tmux` are on `PATH`, proves `gh auth token` fails under the isolated `GH_CONFIG_DIR`, and records the git revision, dirty source file count and binary hashes in `meta.txt`.
   `verify-depot.sh doctor` runs this step alone and cleans up; run it first whenever anything looks off.
3. Fixture.
   Writes `config.toml` with one profile, `verify-haiku` (Claude, `haiku`, effort `low`), and creates `<throwaway>/verify-demo` with one commit pushed to `<throwaway>/remote.git` and a `.depot.toml` mapping `build` and `fix` to that profile.
4. Drive.
   Runs the feature's recipe from its file under `features/`, asserting exit codes, stdout, stderr, files and journal rows after every command.
5. Cleanup.
   Stops every `depotd` it started, kills its tmux server, deletes the throwaway by exact path, then asserts the throwaway is gone and the evidence is still there.

Exit 0 means every assertion held.
Exit 1 prints the step and the reason and leaves the evidence behind.
Exit 2 is a guard refusal or a usage error.

## Evidence

Evidence lives in `~/.depot-verify/runs/<UTC stamp>-<feature>/`, outside the repo, the throwaway, `~/.agni` and `~/.depot`, so it survives cleanup.

- `meta.txt`: doctor facts, the throwaway paths, every daemon pid, and whether the throwaway was removed.
- `transcript.txt`: every command in order with its exit code, stdout (`out|`) and stderr (`err|`).
- `commands/NN-<label>.{cmd,out,err,exit}`: the same, one file set per command.
- `build.log`: the cargo build output.
- `frames/NN-<label>.{ansi,png}`: one frame per command (the command line, its output and exit code) and one per TUI pane capture, in run order.
- `proof.mp4`: the frames stitched two seconds apiece into a 1280x800 video, ready to attach to a PR.
  It needs `python3`, `ffmpeg` and a headless `google-chrome` or `chromium`; without them the run still passes and `meta.txt` records `video: skipped: <reason>`.
  `scripts/render-frame.py <frame.ansi> <frame.png>` renders one frame by hand.
- Per feature: `checklist.md`, `events.txt` (the journal, read with `sqlite3 -readonly`), `task-states*.txt`, `docs/`, `tui-*.txt` pane captures, `depotd-*.log` and `depotd-lock-1.json`.

Proof standards:

- Drive the CLI and the daemon binary a user runs.
  Never write to `agni.db` or the checklist by hand, and never call internal functions.
- Capture the command and the resulting state.
  A `stopped t-1` line alone is not proof; the journal row and the checklist section are.
- Check side effects next to the output: `.git/info/exclude`, the project store tree, the docs files, the lock file, the journal, and the absence of any boxr session or treehouse lease.
- The only stand-ins are the ones production already isolates: a local bare remote instead of GitHub, and a placeholder token so the forge refuses rather than accepts.

## Cleanup

The driver cleans up on every exit path, including failures and Ctrl-C.
If a run was killed hard (SIGKILL, a closed terminal) and left a throwaway or a daemon behind, clean it from its evidence directory:

```
.claude/skills/verify-depot/scripts/verify-depot.sh cleanup ~/.depot-verify/runs/<run-id>
```

That stops only the pids listed as `daemonPid` in `meta.txt` whose command line is still the recorded `depotd` binary, kills the recorded tmux socket, and removes the recorded throwaway if it matches `depot-verify.*`.
It never touches the evidence.

## Feature map

`features/README.md` is the index and the contract for a feature file.

- [project-add](features/project-add.md): `depot project add`, the store scaffold, `.git/info/exclude`, re-add, the unmapped profile refusal.
- [task-lifecycle](features/task-lifecycle.md): `task add --role`, dispatch refusal without a Typesafe key, approve, stop, acknowledge, retry, `depot inbox`, the worker-context guard.
- [status-tui](features/status-tui.md): `depot status`, `--history`, and `depot status --tui` in tmux.
- [doc-write](features/doc-write.md): `depot doc write` inline, from stdin, `context.md`, and the path escape refusal.
- [daemon](features/daemon.md): `depotd` credential refusal, lock record, single-instance lock, heartbeat, restart recovery, and no worker launched for held work.
- [worker-loop](features/worker-loop.md): approve, lease, real boxr worker, submit, validate, push.
  Spends quota, not wired into the driver.

## Known gaps

- The full worker loop needs a real paid boxr session, so the driver does not run it.
  `features/worker-loop.md` has the gated manual recipe.
- Forge delivery (open PR, checks, merge, evidence comments) cannot be exercised without the real GitHub API, and depot has no forge base URL setting to point elsewhere.
  It is not verified here.
- Dispatch without `--role` needs the Typesafe network service and a real key.
  Only the refusal without a key is verified.
- `depot ask`, `depot submit`, `task answer`, `task redirect`, `task rework` and `task release` need a running worker or a leased worktree, so they belong to the worker loop.

## Gotchas

- An approved task plus a running `depotd` launches a real worker within one poll.
  Never approve while the daemon runs unless you mean to spend quota.
- `depot task stop` after `depot task retry` prints `stopped` and exits 0 but journals nothing: the `task_cancelled:<id>` event key is not attempt-scoped, so the second cancel collides with the first and is dropped.
  The task stays running, and a daemon will launch it.
  Do not rely on stop-after-retry to cancel.
- `depotd` writes its pid and heartbeat to `run/depotd.scope.json`; `run/depotd.lock` stays empty.
- `depotd --help` prints its usage on stderr prefixed `depotd:` and exits 1.
- A project's store name is its slug, taken from the directory name, so the fixture store is `<throwaway>/home/projects/verify-demo`.
- `depot doc write <name>` writes `docs/<name>` literally.
  `context` and `context.md` are different files.
