# One depotd per store serves every project

Until now a daemon was launched per project (`depotd --project <path>`), but the
instance lock was already store-global, so one idle daemon starved every other
project on the machine: the lock let only one run while that one served a single
project.
The observed failure was a project waiting 15 hours behind a daemon that was
pinned to a different project.

## The singleton and the server are the same daemon

The lock was right and the process shape was wrong.
One depotd now serves every project registered in the store: each tick walks the
projects, builds the per-project daemon over the shared store and adapters, and
reconciles that project.
`--project` survives as an optional debug filter that narrows the loop to one
project, still under the same store lock, so debugging a project replaces the
service rather than racing it.
No state changes: the store was already keyed by project, and every fact and
reconciliation was already per project.

## The concurrency cap splits into a global cap and a per-project cap

With one daemon serving all projects, the configured `concurrency` becomes the
store-global cap on in-flight tasks; a per-project cap under it keeps one busy
project from taking every slot.
The cap a project sees is its own cap clamped to the global slots the other
projects have not taken, computed from the store's records each time a project
state is built, so the CLI and the daemon agree on it and the computation is a
pure function of stored facts and settings.
Defaults: the global cap comes from the existing `concurrency` key, and each
project gets 1 unless `project_concurrency` in the machine-local `config.toml`
raises it.

The rotation is the fairness mechanism: the daemon walks the projects in an order
that shifts by one each tick, so when global slots are scarce, the project that
went last this tick goes first next tick.
This is scheduling configuration, not a fact: no new fact kind, no new state.
