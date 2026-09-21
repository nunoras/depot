# Doc write

`depot doc write` lets the coordinator keep durable notes in the project store's `docs/` directory, from an inline value or from stdin, including the scaffolded `context.md`, while refusing any name that escapes the store.

## Sub-features

- `doc-inline` writes `--content <text>` to `docs/<name>`.
- `doc-stdin` writes `--content -` from stdin and creates nested directories.
- `doc-context` replaces the scaffolded `docs/context.md` from inside the store.
- `doc-escape` refuses a name with `..` and writes nothing.
- `doc-no-checklist` leaves the checklist untouched.

## How to get to it (user POV)

- Run `depot doc write <name> --content <text|-> [--project <name>]` from the repo or from its store.

## Driving it with verify-depot.sh

Preconditions:

- The fixture is registered, so `<throwaway>/home/projects/verify-demo/docs/context.md` holds the scaffold stub.

- **Run it.** Run `.claude/skills/verify-depot/scripts/verify-depot.sh doc-write`.
- **Inline.** `depot doc write notes.md --content "first line"` in the repo.
  Stdout `wrote <store>/docs/notes.md`, and the file holds `first line`.
- **Stdin.** `depot doc write plans/piped.md --content -` with two lines on stdin.
  Exit 0, and `docs/plans/piped.md` is byte-identical to the input.
- **Context.** `depot doc write context.md --content "# Context ..."` run from the store root.
  Stdout `wrote <store>/docs/context.md`, and the file holds the new text.
- **Escape.** `depot doc write ../escape.md --content x`.
  Exit 1, stderr `is not a document name inside the store`, and `<store>/escape.md` does not exist.
- **Proof.** The checklist hash is unchanged across all writes, and `docs/` is copied into the evidence directory.

## Gotchas

- The name is a literal file name.
  `depot doc write context` makes `docs/context`, not `docs/context.md`.
- `doc write` is not guarded against worker contexts, unlike `task` and `inbox`.
- Run from outside a registered repo or store, it needs `--project`.
