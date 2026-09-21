# One daemon serves every project, scheduled under a store-global cap

Depot launched one daemon per project, each named with `--project`.
The store lock already spanned the depot home, so the second project's daemon refused to start while the first held it, and one busy project starved every other project registered in the same store.
This ADR records the two decisions that replaced that shape: one daemon serves every project in the store, and the worker cap is shared across projects.

## One process, many projects

The daemon takes no required project.
It serves every project in the store, and `--project` survives only as a debug filter that narrows a run to one registered project.
The singleton guarantee stays where it was: the exclusive lock on `$DEPOT_HOME/depotd.lock` is store-global, so the lock, not the filter, is what makes the daemon single.
Per-project behavior is unchanged: each tick builds a per-project daemon over the shared adapters, and every reconcile pass still derives its pending work from that project's records.

## The worker cap is store-global, the queue is per-project

`concurrency` in the depot home's `config.toml` was already the cap the pure core applied per project; it is now the cap on workers across the whole store.
Each project adds `max_concurrent_tasks` to its machine-local `.depot.toml`, default `1`, bounding how many store slots one project may hold at once, and the effective per-project limit is that number clamped under the store cap.

The split follows the configuration split the project already keeps: how many workers this machine runs lives in the depot home's `config.toml`, and how much of the machine one project may claim lives in the project's `.depot.toml`, which stays machine-local and out of version control.

## Round-robin across projects

The pure core starts queued work per project, so a task becomes `Running` the moment its project has a free slot, even when the store has none.
The daemon therefore owns admission: a tick opens with a budget of free store slots, counted as the cap minus the tasks whose attempts hold worktrees, and the `AcquireWorktree` action consumes from that budget or is left for the next tick.
Reconciliation already retries unstarted work from the records, so a declined admission is a delay, never a lost task.
Projects are served in rotated order, first place advancing one project per tick, so when slots are scarce the project that waited longest through the rotation is served first and one busy project cannot starve the rest.
