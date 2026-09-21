# Status and the TUI

`depot status` renders the project checklist from depot's records, `--history` adds finished tasks, and `depot status --tui` shows the same projects live, refreshing every two seconds, with `h` toggling history and `q` quitting.

## Sub-features

- `status-plain` prints the checklist for the current project.
- `status-history` adds cancelled, failed and landed tasks.
- `tui-live` shows active tasks and a count of hidden finished ones.
- `tui-history` shows finished tasks after `h`.
- `tui-quit` exits cleanly on `q`.

## How to get to it (user POV)

- Run `depot status` or `depot status --history` in a registered repo, or add `--project <name>` or `--all`.
- Run `depot status --tui` in a terminal and press `h`, `q`, arrows or PageUp and PageDown.

## Driving it with verify-depot.sh

Preconditions:

- The fixture is registered.
- `tmux` is on `PATH`.
  The driver starts its own server with `tmux -L depot-verify-<pid>`.

- **Run it.** Run `.claude/skills/verify-depot/scripts/verify-depot.sh status-tui`.
- **Seed.** The driver adds `Held for approval` (`t-1`), adds `Cancelled early` (`t-2`), then approves and stops `t-2`.
  `depot status --history` lists both titles.
- **Live view.** The driver starts `depot status --tui` in a 140x40 tmux pane in the repo and waits for the header `refresh 2s  h history  q quit`.
  The pane lists `verify-demo/t-1 (build) proposed` and `Held for approval`, shows `1 cancelled  h shows history`, and does not list `Cancelled early`.
- **History.** The driver sends `h`.
  The pane shows `history on  h back` and lists `verify-demo/t-2 (build) cancelled` with `Cancelled early`.
- **Quit.** The driver sends `q` and waits for the tmux session to end.
- **Proof.** `tui-live.txt` and `tui-history.txt` hold the two pane captures, and `meta.txt` records `tuiExitedOnQ: yes`.

## Gotchas

- The TUI uses the alternate screen and raw mode.
  Always drive it inside tmux, never on the agent's own PTY.
- The first frame renders after one collect, so wait for the header rather than sleeping.
- A running task with a session makes the TUI call `boxr status <session>` for its step count.
  With the throwaway `BOXR_HOME` that finds nothing, which is fine.
- The plain `no daemon is driving this project` line only appears when a task is approved or in flight.
  A project with only held tasks never shows it, daemon or not.
