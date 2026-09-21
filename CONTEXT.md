# depot

The language depot uses for a project's agent work.

## Work

**Project**:
One repository whose agent work depot coordinates.
A single depot home holds many projects, and no project's records leak into another's.
_Avoid_: workspace, repo

**Task**:
One unit of work, with a role and a state and its own history of attempts, questions and validations.
_Avoid_: job, ticket, issue

**Role**:
What kind of work a task is: plan, build, review or fix.
A role resolves to a worker profile through project configuration, so no model is named in code.
_Avoid_: agent type, worker kind

**Attempt**:
One worker session on a task.
A task may have several across retries, pauses and rework.
_Avoid_: run, execution

**Brief**:
What one worker is told for one task, rendered from the task record: intent, role, output destination, done criteria, dependencies, validation, the store, and the two worker calls.
It is handed to the worker when its session launches.
_Avoid_: prompt, instructions

**Dependency edge**:
A task's requirement on another task, naming that prerequisite and the exact commit its validation was bound to.
The pin is what a dependent started against, so it is also what can go stale.
_Avoid_: blocker, parent

**Worktree lease**:
The pooled worktree an attempt runs in.
A lease holding unlanded work is never reset or removed.
_Avoid_: checkout

## State

**Fact**:
Something the daemon observed, carrying the time it observed it.
Facts are the only input the lifecycle accepts.
_Avoid_: event, signal

**Action**:
Something the daemon intends to do as a consequence of a fact.
_Avoid_: command, task

**Reduce**:
The step from a project state and a fact to the next project state and the actions to take.
_Avoid_: transition function, handler

**Validation record**:
The result of running a project's validation command at one exact commit: the command, the commit, the exit code, the duration and the output tail.
A pass is bound to that commit alone, so a moved branch invalidates it.
_Avoid_: test run, check result

**Validated**:
A task holds a passing validation record for the commit its branch currently sits at.
_Avoid_: green, passing

**Landed**:
A task whose pull request merged, so its work is on the default branch and its worktree returns to the pool.
_Avoid_: merged, done

**Failed**:
A task that stopped and needs a person, with its branch, worktree and validation output kept for review.
Failing validation, overrunning the run duration and exhausting retries all end here.
_Avoid_: paused, errored

**Checklist**:
The rendered view of a project's tasks, drawn from the records and never edited by hand.
_Avoid_: status board, dashboard

**Artifact**:
A file depot copies into the depot home and hands to the machine's configured publish command, which returns the one URL that stands for it.
depot keeps the copy and never hosts or serves it.
_Avoid_: attachment, upload, asset

**Build id**:
The git commit a binary was built from, carried by both `depot` and `depotd` and reported by `--version`.
A daemon whose build id differs from the client's is stale, and `depot status` says so.
_Avoid_: version tag, revision

## People

**Coordinator**:
The one session per project that the user talks to.
It turns conversation into tasks, answers only the questions the brief already settles, and never edits project code.

**Policy prompt**:
The versioned statement of what the coordinator owns, shipped with every new coordinator session.
_Avoid_: system prompt

**Kickoff**:
The first message of a coordinator session: the store, the live checklist and the context document, and the command that reads what changed since the previous turn.
_Avoid_: greeting, bootstrap

**Inbox**:
The facts recorded since the coordinator's previous turn, joined to where each task stands now, split by who has to act on them.
It is the first thing a coordinator reads every turn.
_Avoid_: notification, feed

**Context document**:
The coordinator's durable narrative for a project, written into the store and never rendered by depot.
It is what a rotated session reads to catch up.
_Avoid_: memory, notes

**Worker**:
A headless session with a narrow contract: change code, ask a question, or submit.
Everything else it writes is commentary.

**User**:
The single person running depot.
_Avoid_: captain, operator, customer

## Limits

**Cap**:
The most tasks a project may have in flight at once.
Work beyond it queues and starts when a slot frees.
_Avoid_: concurrency limit

**Fallback profile**:
An explicitly configured alternative worker profile.
Switching profile happens only through that list.
_Avoid_: alternative model

**Relayed question**:
A question that reaches the user, either because the coordinator judged it outside the brief or because the project relays every question.
A question the coordinator settles itself does not block the task.
