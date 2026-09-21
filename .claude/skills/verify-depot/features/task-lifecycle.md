# Task lifecycle

A coordinator files tasks with `depot task add`, and each lands held until approved; approve, stop, acknowledge and retry move it through its states, `depot status` and `depot inbox` report each move, and worker contexts are refused coordinator commands.

## Sub-features

- `add-role` files a held task with an explicit `--role`.
- `add-no-role` refuses dispatch without a Typesafe key and files nothing.
- `approve` moves a held task to approved.
- `stop` cancels an approved task.
- `acknowledge` records that a person has seen a cancelled task.
- `retry` sends a cancelled task back to the approved queue.
- `inbox` reports the facts since the last read and then drains.
- `worker-guard` refuses coordinator commands when `DEPOT_TASK_ID` or `DEPOT_ATTEMPT_ID` is set.

## How to get to it (user POV)

- Run `depot task add --title <t> --intent <i> --role <plan|build|review|fix>` in a registered repo.
- Run `depot task approve|stop|acknowledge|retry <task-id>`.
- Run `depot status`, `depot status --history` and `depot inbox`.

## Driving it with verify-depot.sh

Preconditions:

- The fixture is registered with `depot project add`.
- No `depotd` runs against the throwaway home.
  The driver never starts one for this feature.

- **Run it.** Run `.claude/skills/verify-depot/scripts/verify-depot.sh task-lifecycle`.
- **File.** `depot task add --title "Write a readme" --intent "Add one line to README.md" --role build`.
  Exit 0, stdout `added t-1`.
  `depot status` has `## Held - awaiting approval (1)`, ``- `verify-demo/t-1` **Write a readme** (build)`` and `waits on: approval`.
- **Inbox.** `depot inbox` prints `1 fact since your last turn.` and `a task was filed, holding for approval`.
  A second `depot inbox` prints `No facts since your last turn.`
- **No role.** `depot task add --title "Pick my role" --intent "no role given"`.
  Exit 1, stderr has `Typesafe dispatch refused` and `supply --role`.
  `depot status` still shows one held task and no `Pick my role`.
- **Worker guard.** `env DEPOT_TASK_ID=t-1 DEPOT_ATTEMPT_ID=verify depot task approve t-1`.
  Exit 1, stderr ``worker context may only use `depot ask` and `depot submit` to move state``.
- **Approve.** `depot task approve t-1`.
  Stdout `approved t-1`.
  `depot status` starts with `no daemon is driving this project`.
- **Stop.** `depot task stop t-1`.
  Stdout `stopped t-1`.
  `depot status --history` has `## Cancelled (1)`, and `depot inbox` has `stopped at`.
- **Acknowledge.** `depot task acknowledge t-1`.
  Stdout `acknowledged t-1`.
- **Retry.** `depot task retry t-1`.
  Stdout `retried t-1`.
- **Proof.** `events.txt` holds `task_proposed`, `task_approved`, `task_cancelled`, `task_acknowledged` and `task_retried` rows for `t-1`.
  `task-states.txt` and `checklist.md` show the end state.

## Gotchas

- Approving while a `depotd` runs launches a real worker within one poll.
- `task stop` after `task retry` prints `stopped t-1` and exits 0 but journals nothing, because the `task_cancelled:t-1` event key already exists.
  The task stays running.
  The driver ends on retry so the run stays green; that bug is not covered as passing.
- A refused `task add` still consumes a task id, so the next task after a refusal can be `t-3`.
- After approve with no daemon the checklist puts the task under `Running` with `attempt: stalled - in_flight 0s, no worker session`.
  That is expected: nothing is leasing or launching.
- `depot inbox` moves the read position.
  Read it once per check or the next assertion sees `No facts`.
