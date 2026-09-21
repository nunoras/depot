# First real project report (boxr)

Subject: `nunoras/boxr` on main (0.2.0, DuckDB dropped, pi exit facts/resume from PR 40).
Date of run: 2026-09-18.
Depot worktree: this branch.

## What the run proved

- `depot` and `depotd` install from this repo onto PATH and stay usable.
- boxr registers as a path project with a real committed `.depot.toml` (profiles, validation `cargo test`, dispatch rules, `pull_request.auto_merge`).
- Machine-local profiles can drive pi + xAI with an empty account (pi has no isolated account dir).
- One real task (`t-2`) ran end to end with no fakes:
  - treehouse lease under `~/.treehouse/boxr-b084e1`
  - boxr launch (`--harness pi --model xai/grok-4.5 --effort off --kind build --detach`)
  - worker committed `.depot.toml` and called `depot submit`
  - daemon validated at commit `f7e80bc…` (`cargo test` exit 0)
  - daemon pushed `depot-t-2` and opened [https://github.com/nunoras/boxr/pull/41](https://github.com/nunoras/boxr/pull/41)
  - daemon observed `pull_request_merged` after the PR landed and released the worktree
- Forge polling stayed confined to `PrOpen` and recorded check state changes.
- `docs/boxr-contract.md` matches settled boxr 0.2.0 status/ps/wait (including wait exit 0 for failed/interrupted/timeout-running).

## Depot fixes this run forced

1. Launch `--kind` is the task role (`build` / `plan` / `review` / `fix`), not the invalid boxr kind `worker`.
2. `--account` is omitted when the profile account is empty (required for pi).
3. Nested boxr TOON (`session:` / `id` / `state` / `status`) is accepted via leaf-key reads; regression test added.
4. Treehouse acquire always places the lease on a delivery branch, including `DefaultBranchHead`. The name now comes from `depot_core::delivery_branch` over the task title and intent, a `feat`/`fix`/`chore`/`refactor` prefix plus a title slug, with a numeric suffix when the name is taken; this run still saw `depot-<task>`.
5. Delivery falls back to creating that same conventional branch when the worktree is still detached, so a restart can finish a validated task. The fallback also produced `depot-<task>` during this run.

## What the run could not prove

- **CI green on the subject PR.** GitHub Actions refused both jobs: account billing / spending limit. Local `cargo test` in the leased worktree passed; forge checks stayed `failing`. Merge was completed with admin merge so depot could still observe land.
- **Automatic merge by depot.** `auto_merge` is on, but auto-merge needs passing checks. With CI blocked by billing, depot never asked GitHub to merge; a person merged and depot observed it.
- **First model choice.** `xai/grok-code-fast-1` rejected `reasoningEffort` and exited in ~2s; liveness then marked the worker gone and failed `t-1` (worktree kept on purpose). `xai/grok-4.5` with `effort = "off"` worked.
- **Coordinator session as a long-lived daily pane.** This run drove the loop from the CLI; the open path is documented in `docs/daily-driver.md` but was not left running as the human's main chat.
- **Typesafe dispatch.** The task used `--role build` and skipped the Typesafe matcher.

## Operator residue on the machine

- `~/.depot/config.toml` holds the grok-* pi profiles used above.
- `depotd --project boxr` may still be running under the process that finished the land.
- boxr main now contains `.depot.toml` via PR 41.
