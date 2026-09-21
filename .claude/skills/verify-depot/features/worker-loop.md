# Worker loop

With the daemon running, an approved task gets a treehouse worktree on a delivery branch named from the task title and intent (a `feat`/`fix`/`chore`/`refactor` prefix plus a title slug), a real boxr worker session on the task's profile, and, when the worker calls `depot submit`, a validation run at the submitted commit followed by a push and a pull request.

## Sub-features

- `lease` acquires a treehouse worktree and journals `worktree_acquired`.
- `launch` starts a boxr session and journals `worker_turn_started`.
- `ask-answer` lets the worker call `depot ask` and the coordinator answer with `depot task answer`.
- `submit-validate` runs `[validation].command` at the submitted commit.
- `push` pushes the branch to `origin`.
- `open-pr` opens a pull request on the forge.
  Not verifiable here.

## How to get to it (user POV)

- Run `depotd`, then `depot task approve <task-id>`.
- Watch with `depot status` and `depot inbox`.

## Driving it with verify-depot.sh

Preconditions:

- This spends real quota and is not wired into the driver.
  Run it by hand at most once per change you need to prove.
- Use the fixture's `verify-haiku` profile (Claude, `haiku`, effort `low`) and a one-line intent.
- Reproduce the exports from the top of `../scripts/verify-depot.sh` in a fresh shell, and keep the throwaway `BOXR_HOME`, `TREEHOUSE_ROOT` and `GH_CONFIG_DIR`.

- **File.** `depot task add --title "Add a line" --intent "Append the line ok to README.md, commit, then run depot submit." --role build`.
- **Start.** Start `depotd` in the background and record `$!`.
- **Approve.** `depot task approve t-1`.
  Within one poll the journal has `worktree_acquired` and `worker_turn_started`, and `<throwaway>/pool/.treehouse/` holds the worktree.
- **Watch.** `BOXR_HOME=<throwaway>/boxr boxr ps` lists the session.
  `depot status` shows the attempt and session.
- **Submit.** After the worker runs `depot submit`, the journal has `worker_submitted`, `validation_started` and `validation_finished`, and `git -C <throwaway>/remote.git branch` lists the pushed branch.
- **Stop.** `BOXR_HOME=<throwaway>/boxr boxr stop <session>` if it is still running, then SIGTERM the `depotd` pid.
- **Proof.** The journal from `sqlite3 -readonly <home>/depot.db`, `boxr show <session>`, and the pushed branch on the bare remote.

## Gotchas

- The pull request step calls the real GitHub API at a fixed base URL.
  With the placeholder token it fails, and there is no setting to point it at a local forge.
  Treat everything after the push as unverified.
- Claude Code writes its own transcript under `~/.claude/projects/` even with a throwaway `BOXR_HOME`.
- `task stop` after `task retry` does not cancel (see `task-lifecycle.md`).
  Stop a retried task by stopping the daemon first.
- Kill the worker through `boxr stop` or its supervisor pid from `boxr ps`, never by process name.
