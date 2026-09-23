---
status: proposed
---

# A project is identified by its origin, not its path

`depot project add` identifies a project by the absolute path it was given, so the same repository cloned at another path, or on another machine, becomes a second project.
Once preferences follow the user between machines, that breaks: a role map set for a repository on Linux never reaches the same repository on Windows.

A project is identified by its origin remote, normalised to host, owner and name.
The path of each clone is a machine fact that maps the identity to a place on one machine.
A repository with no origin is a local-only project whose preferences never sync, and a renamed repository is re-pointed explicitly.

## Considered options

- A generated id committed in `.agni/project.toml`.
  It survives renames, but `project add` would leave a file that must be committed before anything works.

## Normalisation

The identity is the remote named `origin`, reduced to lowercase host, owner and name: the scheme, user, port, a trailing `/` and `.git` are dropped, so `git@github.com:O/R.git`, `https://github.com/o/r/` and `ssh://git@github.com/o/r` are one identity.
A clone whose `origin` is a fork is the fork's project; nothing guesses at an upstream.
Changing `origin` under a registered clone is refused while the project has a task in flight.

## The identity is not a path

A project's store directory keeps a short unique slug, and the database maps the identity to it.
A path built from host, owner and name would differ in case between Linux and Windows, could not hold a port on Windows, would push `scratch/` past the Windows path limit, and would turn a rename into a directory move under a running daemon.
