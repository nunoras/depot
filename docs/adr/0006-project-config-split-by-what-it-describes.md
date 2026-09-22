---
status: proposed
---

# Project knowledge is split by what it describes, and the repo half is committed

Until now all project configuration lived in `.depot.toml`, machine-local and kept out of version control, which `crates/depotd/tests/config_split.rs` guards.
That makes the validation gate drift from the code it gates: a repo that changes its test command changes CI in the same pull request, while the gate in the machine-local file silently stays old.
It also has to be retyped on every machine that runs the project, including a remote run host.

We split project knowledge by what it describes.
What describes the repo is committed under `.agni/`: `project.toml` holds the validation gate, base branch, evidence command, pull request describe style, the dispatch rules' `when` strings and the delivery mode, and `automations/` holds one file per automation.
The verify skill and `AGENTS.md` stay where the harnesses discover them.
What describes this machine's choices for the repo stays in the home, per project: the role-to-profile map, concurrency, and whether auto-merge is on.

## Considered options

- Keep everything machine-local, as today.
  Rejected because the gate drifts and every machine needs the same retyped config.
- Commit everything.
  Rejected because profiles name models and accounts that exist only on one machine, and would leak that setup into public repositories.

## Consequences

- `config_split.rs` has to be rewritten to guard the new line rather than deleted.
- The one home is `~/.agni`, shared by depot, boxr and agni (agni ADR 0001, https://github.com/nunoras/agni/blob/main/docs/adr/0001-one-home-for-depot-boxr-and-agni.md), so nothing in the repo half may point into it.
- User preferences may later sync through agni's cloud so they follow a login across machines.
  That moves where the home half is stored, not where the line between the halves falls.
- The home half is itself three kinds: preferences, which follow the user and may sync; machine facts, which never leave the machine; and secrets, which never travel as preferences.
  Profiles are preferences, and a profile whose harness is missing is shown as unavailable on that machine.

## The gate is read from the base, never from the branch

A committed gate is in the branch a worker edits, so a worker could set its own gate to `true` and pass.
Validation reads `.agni/` from the fetched base, never from the submitted commit, and a task whose branch changes anything under `.agni/` is held for a person.
The gate still moves with the code, one reviewed merge later.

## Where the home half is stored

Preferences are records in the home's database with depot as their only writer; the agni settings screen and `depot preferences export` and `import` go through depot, so a hand edit and a UI save can never overwrite each other.
Nothing in the home is ever synced as files: a sqlite database in WAL mode on a synced or network folder corrupts.
Sync, when it comes, is application-level from the database, and it skips every preference of a local-only project.
On Windows the home is `%USERPROFILE%\.agni`, never `%APPDATA%`, which roaming profiles copy.
boxr keeps its secrets in the same `secrets/` directory as depot, one owner-only file each, so there is one secret store with one permission rule.
