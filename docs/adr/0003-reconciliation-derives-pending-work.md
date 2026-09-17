# Reconciliation derives pending work from records

Two processes apply facts to a task: the daemon, and the CLI the coordinator drives.
`reduce` returns the next state together with the actions that state assumes, and the store writes that next state whatever the caller does with the actions.
The CLI writes facts and executes nothing, because the adapters live in the daemon.

That split is invisible until you follow it end to end.
`depot task approve` persists the task as running with an attempt and no worktree, because starting a task is what the core's approval rule does, while the two actions that make it true - lease a worktree, launch a session - were dropped in the CLI's process.
The task then looks in flight to every reader and no worker will ever exist for it, because the rule that starts a task only looks at approved ones.
The same shape hid in two more places: an answer recorded `ResumeSession` and the worker waited forever, and nothing in the daemon ever read the forge, so a task could open a pull request but never land, release its worktree or leave the published section of the checklist.

The decision is that the daemon derives what is outstanding from the records on every tick, and that this - not the action list of a fact - is what moves work forward.
`reconcile_start` leases and launches an in-flight attempt that has neither, `reconcile_resume` delivers an answer the worker has not been told about, and `reconcile_validation`, `reconcile_delivery` and `reconcile_forge` carry a submitted commit, a validated one and an open pull request to their next state.
Leasing a worktree, launching a worker and resuming a worker record a durable intent before their external call, so recovery surfaces an uncompleted intent rather than repeating it.
Validation, pushing, opening a pull request and releasing a worktree do not yet have that crash boundary, so their adapters must remain idempotent until their own durable intents exist.

The alternative we rejected was to queue the actions a fact produced and have the daemon drain the queue.
A queue is a second source of truth beside the records, it needs its own delivery and its own recovery, and a fact whose action ran but was not yet marked as run is a duplicate side effect: a second lease, a second validation run, a second pull request.
Replaying the journal is the same trap in a different shape, because a fact is only meaningful against the state it arrived in, and re-reducing last week's submission against today's records is not the same computation.

The rule that follows is what a new action has to answer before it ships.
If the daemon can tell from the records that the work is outstanding, it needs a reconcile pass, and the action list of the fact that started it is a shortcut rather than the delivery mechanism.
If only the process that recorded the fact can take the action, that process takes it there and then, the way `depot ask` prints the notification itself.
The end-to-end test for the first kind is the golden path: a step a coordinator's command set in motion has to reach its end with no further help from that process.
