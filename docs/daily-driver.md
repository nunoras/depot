# Daily driver: depot on boxr

How to open depot as the main agent for a real project, with boxr as the launcher.

## One-time install

From a depot checkout:

```
cargo install --locked --path crates/depot
cargo install --locked --path crates/depotd
```

Confirm:

```
depot --help
depotd --help
boxr --version   # needs 0.2.0 or newer
treehouse status
gh auth status
```

## Machine-local settings

`$DEPOT_HOME/config.toml` (default `~/.depot/config.toml`) holds profiles and concurrency.
Account may be empty when the harness has no isolated profiles (pi uses the host credentials).

```toml
concurrency = 1
poll_interval_seconds = 10

[profiles.grok-build]
harness = "pi"
model = "xai/grok-4.5"
effort = "off"
account = ""
```

Map every role the project uses.
Keep concurrency at 1 on a small host.

## Project config

In the project repository, `.depot.toml` is committed knowledge: roles, validation, dispatch, PR base.

```
depot project add /path/to/boxr
```

Edit `.depot.toml` so `[profiles]` names the machine-local profiles above, and `[validation].command` is the real project gate.

## Start the daemon

One daemon per machine lock:

```
depotd --project boxr
```

Leave it running.
It leases worktrees, launches boxr sessions, validates, pushes, opens PRs, and observes merges.

## Start the coordinator session

From the project store or with `--project`:

```
boxr --harness pi --model xai/grok-4.5 --effort off --kind plan --detach "$(cat <<'EOF'
You are the depot coordinator for boxr.
Begin every turn with: depot inbox --project boxr
Then: depot status --project boxr
Policy and kickoff live under the project home in ~/.depot/projects/boxr once you need them; prefer depot doc write for durable notes.
EOF
)"
```

Or launch interactively and paste the kickoff depot prints once coordinator launch helpers are wired into the CLI.
Until then, the policy and kickoff templates are the assets in `assets/coordinator-policy.md` and `assets/coordinator-kickoff.md`.

## Every coordinator turn

1. `depot inbox --project boxr`
2. `depot status --project boxr`
3. File work with `depot task add ...` (role optional when dispatch rules cover it)
4. Ask the human to approve: `depot task approve <id> --project boxr`
5. Answer worker questions with `depot task answer` or relay them

Do not edit project code in the coordinator session.
Do not launch workers by hand.

## Task loop the daemon owns

approve → treehouse lease → boxr launch → worker submit → validate at commit → push branch `depot-<task>` → open PR → observe checks/merge → release worktree.

## See also

- `docs/boxr-contract.md` for the boxr command surface
- `docs/first-real-project-report.md` for what the first end-to-end run proved
