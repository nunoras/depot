# Project add

`depot project add` registers a git repository with depot, scaffolds its project store in the depot home, renders its first checklist, and keeps the repository's `.depot.toml` machine-local by listing it in `.git/info/exclude`.

## Sub-features

- `add-new` registers a repository and prints its store path.
- `add-scaffold` creates `checklist.md`, `docs/context.md`, `archive/`, `scratch/` and `media/` in the store.
- `add-exclude` lists `.depot.toml` once in `.git/info/exclude`.
- `add-again` reports an existing registration without duplicating anything.
- `add-unmapped` refuses a `.depot.toml` whose roles map to profiles missing from `config.toml`, and leaves no store.
- `status-all` lists every registered project.

## How to get to it (user POV)

- Run `depot project add <path-or-url>` from anywhere.
- Run `depot status` from the repository or its store, or `depot status --all` from anywhere.

## Driving it with verify-depot.sh

Preconditions:

- The fixture repo exists with `.depot.toml` and is not yet registered.
- `verify-depot.sh doctor` passes.

- **Register.** Run `.claude/skills/verify-depot/scripts/verify-depot.sh project-add`.
  The driver runs `depot project add <throwaway>/verify-demo`.
  Exit 0, stdout has `registered <repo>`, `home: <throwaway>/home/projects/verify-demo` and `config: .depot.toml stays machine-local, ignored via .git/info/exclude`.
- **Config stays local.** The driver reads `.git/info/exclude` and runs `git status --porcelain --untracked-files=all`.
  `.depot.toml` is listed once in the exclude file and absent from git status.
- **Store scaffold.** The driver checks the store for `checklist.md`, `docs/context.md`, `archive`, `scratch` and `media`.
- **Add again.** The driver reruns `depot project add <repo>`.
  Exit 0, stdout `already registered <repo>`, exclude file still lists `.depot.toml` once.
- **Status.** The driver runs `depot status` in the repo and `depot status --all` in the throwaway root.
  Both print `# Checklist` and `Project: <repo>`.
- **Unmapped profile.** The driver makes `<throwaway>/verify-unmapped` with `build = "missing"` and runs `depot project add` on it.
  Exit 1, stderr names ``role `build` maps to profile `missing` ``, and no `projects/verify-unmapped` store exists.
- **Proof.** `checklist.md` and `git-info-exclude.txt` are copied into the evidence directory next to `transcript.txt`.

## Gotchas

- The project identity is the absolute path passed in, not the remote URL.
  Adding the same repo by another path registers a second project.
- A plain directory with no `.git` is registered without complaint.
  Nothing is excluded because there is no `.git/info/exclude`.
- The slug comes from the last path segment.
  A second repo with the same directory name gets `<name>-2`.
