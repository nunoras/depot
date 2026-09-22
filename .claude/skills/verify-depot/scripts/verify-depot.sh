#!/usr/bin/env bash
set -euo pipefail

skill_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$skill_dir/../../.." && pwd)"
evidence_root="$HOME/.depot-verify"
real_agni_home="$HOME/.agni"
real_depot_home="$HOME/.depot"
features="project-add task-lifecycle status-tui doc-write daemon"

say() { printf '%s\n' "$*"; }

usage() {
  say "usage: $0 doctor"
  say "       $0 <feature>      one of: $features"
  say "       $0 cleanup <run-dir>"
}

refuse() {
  say "verify: refused" >&2
  say "  why: $*" >&2
  exit 2
}

resolved() { realpath -m "$1"; }

inside() {
  local child parent
  child="$(resolved "$1")"
  parent="$(resolved "$2")"
  [ "$child" = "$parent" ] || [ "${child#"$parent"/}" != "$child" ]
}

guard_environment() {
  if [ -n "${DEPOT_TASK_ID:-}" ] || [ -n "${DEPOT_ATTEMPT_ID:-}" ]; then
    refuse "DEPOT_TASK_ID or DEPOT_ATTEMPT_ID is set; this is a depot worker context"
  fi
  if [ -n "${AGNI_HOME:-}" ]; then
    refuse "AGNI_HOME is already set to $AGNI_HOME; run this from a shell without an agni home"
  fi
  if [ -n "${DEPOT_HOME:-}" ]; then
    refuse "DEPOT_HOME is already set to $DEPOT_HOME; run this from a shell without a depot home"
  fi
  [ -f "$repo_root/Cargo.toml" ] && [ -d "$repo_root/crates/depotd" ] ||
    refuse "no depot checkout found above $skill_dir"
}

own_depotd() {
  local pid="$1" binary="$2"
  [ -r "/proc/$pid/cmdline" ] || return 1
  [ "$(tr '\0' '\n' <"/proc/$pid/cmdline" | head -n 1)" = "$binary" ]
}

cleanup_run() {
  local run_dir="$1" meta binary throwaway socket pid
  meta="$run_dir/meta.txt"
  [ -f "$meta" ] || refuse "no meta.txt in $run_dir"
  binary="$(sed -n 's/^depotdBinary: //p' "$meta" | head -n 1)"
  throwaway="$(sed -n 's/^throwaway: //p' "$meta" | head -n 1)"
  socket="$(sed -n 's/^tmuxSocket: //p' "$meta" | head -n 1)"
  for pid in $(sed -n 's/^daemonPid: //p' "$meta"); do
    if own_depotd "$pid" "$binary"; then
      kill -TERM "$pid" 2>/dev/null || true
      say "verify: stopped depotd $pid"
    fi
  done
  if [ -n "$socket" ]; then
    tmux -L "$socket" kill-server 2>/dev/null || true
  fi
  case "$throwaway" in
    "${TMPDIR:-/tmp}"/depot-verify.*) rm -rf "$throwaway" ;;
    "") ;;
    *) say "verify: refusing to remove the unexpected path $throwaway" >&2 ;;
  esac
  say "verify: cleaned $run_dir"
}

case "${1:-}" in
  cleanup)
    [ -n "${2:-}" ] || { usage; exit 2; }
    cleanup_run "$2"
    exit 0
    ;;
  doctor) mode=doctor; feature=doctor ;;
  project-add | task-lifecycle | status-tui | doc-write | daemon) mode=drive; feature="$1" ;;
  *) usage; exit 2 ;;
esac

guard_environment

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run_id="$stamp-$feature"
run_dir="$evidence_root/runs/$run_id"
throwaway="$(mktemp -d "${TMPDIR:-/tmp}/depot-verify.XXXXXX")"
meta="$run_dir/meta.txt"
step=guard
daemon_pids=""
tmux_socket=""
command_count=0
frame_count=0

inside "$throwaway" "$real_agni_home" && refuse "the throwaway $throwaway lands inside $real_agni_home"
inside "$evidence_root" "$real_agni_home" && refuse "the evidence root $evidence_root lands inside $real_agni_home"
inside "$throwaway" "$real_depot_home" && refuse "the throwaway $throwaway lands inside $real_depot_home"
inside "$evidence_root" "$real_depot_home" && refuse "the evidence root $evidence_root lands inside $real_depot_home"
inside "$throwaway" "$HOME/.treehouse" && refuse "the throwaway $throwaway lands inside ~/.treehouse"

export AGNI_HOME="$throwaway/home"
export BOXR_HOME="$throwaway/boxr"
export TREEHOUSE_ROOT="$throwaway/pool"
export GH_CONFIG_DIR="$throwaway/gh"
export GIT_TERMINAL_PROMPT=0
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN TREEHOUSE_LEASE_HOLDER

inside "$AGNI_HOME" "$real_agni_home" && refuse "AGNI_HOME would be $AGNI_HOME"
[ "$(resolved "$AGNI_HOME")" != "$(resolved "$real_agni_home")" ] || refuse "AGNI_HOME resolves to the real home"
inside "$AGNI_HOME" "$real_depot_home" && refuse "AGNI_HOME would be $AGNI_HOME"

note() {
  if [ -f "$meta" ]; then
    printf '%s: %s\n' "$1" "$2" >>"$meta"
  fi
}

fail() {
  say "verify: failed" >&2
  say "  feature: $feature" >&2
  say "  step: $step" >&2
  say "  why: $*" >&2
  say "  evidence: $run_dir" >&2
  exit 1
}

stop_daemon() {
  local pid="$1" waited=0
  own_depotd "$pid" "$depotd_bin" || return 0
  kill -TERM "$pid" 2>/dev/null || true
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 50 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -KILL "$pid" 2>/dev/null || true
  fi
  wait "$pid" 2>/dev/null || true
}

remove_throwaway() {
  local pid
  cd "$repo_root"
  for pid in $daemon_pids; do
    stop_daemon "$pid"
  done
  daemon_pids=""
  if [ -n "$tmux_socket" ]; then
    tmux -L "$tmux_socket" kill-server 2>/dev/null || true
  fi
  [ -e "$throwaway" ] || return 0
  case "$throwaway" in
    */depot-verify.*) rm -rf "$throwaway" || true ;;
    *) say "verify: refusing to remove the unexpected path $throwaway" >&2 ;;
  esac
  if [ -e "$throwaway" ]; then note throwawayRemoved no; else note throwawayRemoved yes; fi
}

on_exit() {
  local code=$?
  remove_throwaway
  exit "$code"
}
trap on_exit EXIT
trap 'fail "interrupted by SIGINT"' INT
trap 'fail "terminated by SIGTERM"' TERM

mkdir -p "$run_dir/commands" "$run_dir/frames"
say "verify: feature $feature"
say "verify: evidence $run_dir"

run_in() {
  local dir="$1" label="$2" base code
  shift 2
  command_count=$((command_count + 1))
  base="$run_dir/commands/$(printf '%02d' "$command_count")-$label"
  printf '(cd %s && %s)\n' "$dir" "$*" >"$base.cmd"
  set +e
  if [ -n "${stdin_file:-}" ]; then
    (cd "$dir" && "$@") <"$stdin_file" >"$base.out" 2>"$base.err"
  else
    (cd "$dir" && "$@") </dev/null >"$base.out" 2>"$base.err"
  fi
  code=$?
  set -e
  printf '%s\n' "$code" >"$base.exit"
  {
    printf '$ %s\n' "$(cat "$base.cmd")"
    printf 'exit: %s\n' "$code"
    sed 's/^/out| /' "$base.out"
    sed 's/^/err| /' "$base.err"
    printf '\n'
  } >>"$run_dir/transcript.txt"
  last="$base"
  last_code="$code"
  frame_command "$label" "$base" "$code"
}

next_frame() {
  frame_count=$((frame_count + 1))
  printf -v frame '%s/frames/%02d-%s.ansi' "$run_dir" "$frame_count" "$1"
}

frame_command() {
  local frame
  next_frame "$1"
  {
    printf '\033[2m$\033[0m %s\n' "$(cat "$2.cmd")"
    cat "$2.out"
    [ -s "$2.err" ] && printf '\033[31m%s\033[0m\n' "$(cat "$2.err")"
    printf '\033[2m[exit %s]\033[0m\n' "$3"
  } >"$frame"
}

frame_pane() {
  local frame
  next_frame "$1"
  tmux -L "$tmux_socket" capture-pane -p -e -t verify >"$frame" 2>/dev/null || true
}

video_blocker() {
  command -v ffmpeg >/dev/null 2>&1 || { printf 'no ffmpeg on PATH'; return; }
  command -v python3 >/dev/null 2>&1 || { printf 'no python3 on PATH'; return; }
  command -v google-chrome >/dev/null 2>&1 || command -v chromium >/dev/null 2>&1 || printf 'no headless chrome on PATH'
}

make_video() {
  local blocker ansi png fitted list="$run_dir/frames/concat.txt" fit_dir="$run_dir/frames/fit"
  blocker="$(video_blocker)"
  if [ -n "$blocker" ]; then
    note video "skipped: $blocker"
    return
  fi
  [ "$frame_count" -gt 0 ] || { note video "skipped: no frames"; return; }
  mkdir -p "$fit_dir"
  : >"$list"
  for ansi in "$run_dir"/frames/*.ansi; do
    png="${ansi%.ansi}.png"
    python3 "$skill_dir/scripts/render-frame.py" "$ansi" "$png" >/dev/null || fail "could not render $ansi"
    fitted="$fit_dir/$(basename "$png")"
    ffmpeg -loglevel error -y -i "$png" \
      -vf "scale=1280:800:force_original_aspect_ratio=decrease,pad=1280:800:0:0:color=0x161616" "$fitted" ||
      fail "could not fit $png"
    printf "file '%s'\nduration 2\n" "$fitted" >>"$list"
  done
  printf "file '%s'\n" "$fitted" >>"$list"
  ffmpeg -loglevel error -y -f concat -safe 0 -i "$list" -vf format=yuv420p -r 30 -movflags +faststart \
    "$run_dir/proof.mp4" || fail "ffmpeg could not stitch $list"
  rm -rf "$fit_dir" "$list"
  note video "$run_dir/proof.mp4"
}

expect_exit() { [ "$last_code" = "$1" ] || fail "$(cat "$last.cmd") exited $last_code, expected $1; read $last.err"; }
expect_out() { grep -qE -- "$1" "$last.out" || fail "stdout of $(cat "$last.cmd") lacks /$1/; read $last.out"; }
expect_err() { grep -qE -- "$1" "$last.err" || fail "stderr of $(cat "$last.cmd") lacks /$1/; read $last.err"; }
expect_no_out() { ! grep -qE -- "$1" "$last.out" || fail "stdout of $(cat "$last.cmd") has /$1/; read $last.out"; }

events() { sqlite3 -readonly -separator ' | ' "$AGNI_HOME/agni.db" 'select id, kind, coalesce(task_id, ""), key from events order by id'; }
task_states() { sqlite3 -readonly -separator ' ' "$AGNI_HOME/agni.db" 'select id, state from tasks order by id'; }

step=doctor
say "verify: building depot and depotd from $repo_root"
(cd "$repo_root" && cargo build --bin depot --bin depotd) >"$run_dir/build.log" 2>&1 ||
  fail "cargo build failed; read $run_dir/build.log"
depot_bin="$repo_root/target/debug/depot"
depotd_bin="$repo_root/target/debug/depotd"
[ -x "$depot_bin" ] && [ -x "$depotd_bin" ] || fail "no depot or depotd under $repo_root/target/debug"
export PATH="$repo_root/target/debug:$PATH"
[ "$(command -v depot)" = "$depot_bin" ] || fail "depot on PATH is $(command -v depot), not the build under test"

for tool in git sqlite3 boxr treehouse gh tmux realpath; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is not on PATH"
done
boxr_version="$(boxr --version 2>&1 | head -n 1)"
case "$boxr_version" in
  *" 0.0."* | *" 0.1."*) fail "boxr is $boxr_version; depot needs 0.2.0 or newer" ;;
esac

git_head="$(git -C "$repo_root" rev-parse HEAD)"
git_dirty="$(git -C "$repo_root" status --porcelain -- crates Cargo.toml Cargo.lock assets | wc -l | tr -d ' ')"
{
  printf 'feature: %s\n' "$feature"
  printf 'runId: %s\n' "$run_id"
  printf 'gitHead: %s\n' "$git_head"
  printf 'gitDirtySourceFiles: %s\n' "$git_dirty"
  printf 'depotBinary: %s\n' "$depot_bin"
  printf 'depotBinarySha256: %s\n' "$(sha256sum "$depot_bin" | cut -c1-64)"
  printf 'depotdBinary: %s\n' "$depotd_bin"
  printf 'depotdBinarySha256: %s\n' "$(sha256sum "$depotd_bin" | cut -c1-64)"
  printf 'boxr: %s (%s)\n' "$(command -v boxr)" "$boxr_version"
  printf 'treehouse: %s\n' "$(command -v treehouse)"
  printf 'throwaway: %s\n' "$throwaway"
  printf 'AGNI_HOME: %s\n' "$AGNI_HOME"
  printf 'BOXR_HOME: %s\n' "$BOXR_HOME"
  printf 'TREEHOUSE_ROOT: %s\n' "$TREEHOUSE_ROOT"
  printf 'GH_CONFIG_DIR: %s\n' "$GH_CONFIG_DIR"
} >"$meta"

mkdir -p "$GH_CONFIG_DIR" "$BOXR_HOME"
run_in "$throwaway" gh-token-isolated gh auth token
[ "$last_code" != 0 ] || fail "gh still finds a GitHub token under the isolated GH_CONFIG_DIR; depotd would use a real forge credential"
run_in "$throwaway" depot-help depot --help
expect_exit 0
expect_out '^  depot project add <path-or-url>$'
run_in "$throwaway" depotd-help depotd --help
expect_err 'depotd \[--project <project>\]'
run_in "$throwaway" boxr-help boxr --help
expect_exit 0

if [ "$mode" = doctor ]; then
  step=cleanup
  remove_throwaway
  [ ! -e "$throwaway" ] || fail "the throwaway still exists at $throwaway"
  say "verify:"
  say "  feature: doctor"
  say "  result: ok"
  say "  build: $git_head (dirty source files: $git_dirty)"
  say "  boxr: $boxr_version"
  say "  evidence: $run_dir"
  exit 0
fi

step=fixture
remote="$throwaway/remote.git"
project="$throwaway/verify-demo"
slug=verify-demo
store="$AGNI_HOME/projects/$slug"
mkdir -p "$AGNI_HOME"
cat >"$AGNI_HOME/config.toml" <<'EOF'
concurrency = 1
poll_interval_seconds = 1

[profiles.verify-haiku]
harness = "claude"
model = "haiku"
effort = "low"
account = ""
EOF
git init -q --bare -b main "$remote"
git init -q -b main "$project"
printf '# verify demo\n' >"$project/README.md"
git -C "$project" add README.md
git -C "$project" -c user.name=depot-verify -c user.email=verify@depot.invalid -c commit.gpgsign=false commit -q -m "verify baseline"
git -C "$project" remote add origin "$remote"
git -C "$project" push -q origin main
cat >"$project/.depot.toml" <<'EOF'
base_branch = "main"

[profiles]
build = "verify-haiku"
fix = "verify-haiku"

[validation]
command = "true"

[pull_request]
base = "main"
merge = "manual"
EOF
note project "$project"
note remote "$remote"

add_fixture_project() {
  run_in "$project" project-add depot project add "$project"
  expect_exit 0
}

drive_project_add() {
  run_in "$project" project-add depot project add "$project"
  expect_exit 0
  expect_out "^registered $project\$"
  expect_out "^home: $store\$"
  expect_out '^config: \.depot\.toml stays machine-local, ignored via \.git/info/exclude$'
  [ "$(grep -cx '\.depot\.toml' "$project/.git/info/exclude")" = 1 ] || fail ".git/info/exclude does not list .depot.toml exactly once"
  run_in "$project" git-status git status --porcelain --untracked-files=all
  expect_exit 0
  expect_no_out 'depot\.toml'
  for path in checklist.md docs/context.md archive scratch media; do
    [ -e "$store/$path" ] || fail "the project store lacks $path"
  done

  run_in "$project" project-add-again depot project add "$project"
  expect_exit 0
  expect_out "^already registered $project\$"
  [ "$(grep -cx '\.depot\.toml' "$project/.git/info/exclude")" = 1 ] || fail "a second add listed .depot.toml twice in .git/info/exclude"

  run_in "$project" status depot status
  expect_exit 0
  expect_out '^# Checklist$'
  expect_out "^Project: $project\$"
  run_in "$throwaway" status-all depot status --all
  expect_exit 0
  expect_out "^Project: $project\$"

  local unmapped="$throwaway/verify-unmapped"
  git init -q -b main "$unmapped"
  printf '[profiles]\nbuild = "missing"\n' >"$unmapped/.depot.toml"
  run_in "$unmapped" project-add-unmapped depot project add "$unmapped"
  expect_exit 1
  expect_err 'role `build` maps to profile `missing`'
  [ ! -e "$AGNI_HOME/projects/verify-unmapped" ] || fail "a refused add left a store behind"

  cp "$store/checklist.md" "$run_dir/checklist.md"
  cp "$project/.git/info/exclude" "$run_dir/git-info-exclude.txt"
}

drive_task_lifecycle() {
  add_fixture_project
  run_in "$project" task-add depot task add --title "Write a readme" --intent "Add one line to README.md" --role build
  expect_exit 0
  expect_out '^added t-1$'
  run_in "$project" status-held depot status
  expect_out '^## Held - awaiting approval \(1\)$'
  expect_out "^- \`$slug/t-1\` \*\*Write a readme\*\* \(build\)\$"
  expect_out 'waits on: approval'
  expect_no_out 'no daemon is driving'

  run_in "$project" inbox-first depot inbox
  expect_exit 0
  expect_out '^1 fact since your last turn\.$'
  expect_out 'a task was filed, holding for approval'
  run_in "$project" inbox-drained depot inbox
  expect_out '^No facts since your last turn\.$'

  run_in "$project" task-add-no-role depot task add --title "Pick my role" --intent "no role given"
  expect_exit 1
  expect_err 'Typesafe dispatch refused'
  expect_err 'supply --role'
  run_in "$project" status-after-refusal depot status
  expect_out '^## Held - awaiting approval \(1\)$'
  expect_no_out 'Pick my role'

  run_in "$project" worker-guard env DEPOT_TASK_ID=t-1 DEPOT_ATTEMPT_ID=verify depot task approve t-1
  expect_exit 1
  expect_err 'worker context may only use `depot ask` and `depot submit`'

  run_in "$project" task-approve depot task approve t-1
  expect_exit 0
  expect_out '^approved t-1$'
  run_in "$project" status-approved depot status
  expect_out '^no daemon is driving this project$'

  run_in "$project" task-stop depot task stop t-1
  expect_exit 0
  expect_out '^stopped t-1$'
  run_in "$project" status-history depot status --history
  expect_out '^## Cancelled \(1\)$'
  run_in "$project" inbox-stopped depot inbox
  expect_out 'stopped at'

  run_in "$project" task-acknowledge depot task acknowledge t-1
  expect_exit 0
  expect_out '^acknowledged t-1$'

  run_in "$project" task-retry depot task retry t-1
  expect_exit 0
  expect_out '^retried t-1$'

  events >"$run_dir/events.txt"
  task_states >"$run_dir/task-states.txt"
  for kind in task_proposed task_approved task_cancelled task_acknowledged task_retried; do
    grep -q " | $kind | t-1 | " "$run_dir/events.txt" || fail "the journal has no $kind fact for t-1; read $run_dir/events.txt"
  done
  cp "$store/checklist.md" "$run_dir/checklist.md"
}

pane() { tmux -L "$tmux_socket" capture-pane -p -t verify 2>/dev/null; }

wait_for_pane() {
  local pattern="$1" out="$2" waited=0
  while [ "$waited" -lt 100 ]; do
    pane >"$out" || true
    grep -qE -- "$pattern" "$out" && return 0
    sleep 0.1
    waited=$((waited + 1))
  done
  fail "the TUI never showed /$pattern/; read $out"
}

drive_status_tui() {
  add_fixture_project
  run_in "$project" task-add-held depot task add --title "Held for approval" --intent "stay held" --role build
  expect_exit 0
  run_in "$project" task-add-cancelled depot task add --title "Cancelled early" --intent "stopped" --role build
  expect_exit 0
  run_in "$project" task-approve depot task approve t-2
  expect_exit 0
  run_in "$project" task-stop depot task stop t-2
  expect_exit 0
  run_in "$project" status-plain depot status --history
  expect_out 'Held for approval'
  expect_out 'Cancelled early'

  tmux_socket="depot-verify-$$"
  note tmuxSocket "$tmux_socket"
  tmux -L "$tmux_socket" -f /dev/null new-session -d -s verify -x 140 -y 40 -c "$project" \
    "env AGNI_HOME='$AGNI_HOME' BOXR_HOME='$BOXR_HOME' PATH='$PATH' depot status --tui" ||
    fail "tmux could not start the TUI session"
  wait_for_pane 'q quit' "$run_dir/tui-live.txt"
  frame_pane tui-live
  grep -q 'Held for approval' "$run_dir/tui-live.txt" || fail "the live TUI does not list the held task; read $run_dir/tui-live.txt"
  grep -q 'Cancelled early' "$run_dir/tui-live.txt" && fail "the live TUI lists a cancelled task before history is toggled; read $run_dir/tui-live.txt"

  tmux -L "$tmux_socket" send-keys -t verify h
  wait_for_pane 'Cancelled early' "$run_dir/tui-history.txt"
  frame_pane tui-history

  tmux -L "$tmux_socket" send-keys -t verify q
  local waited=0
  while tmux -L "$tmux_socket" has-session -t verify 2>/dev/null && [ "$waited" -lt 50 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  tmux -L "$tmux_socket" has-session -t verify 2>/dev/null && fail "the TUI did not exit on q"
  note tuiExitedOnQ yes
}

drive_doc_write() {
  add_fixture_project
  local checklist_before
  checklist_before="$(sha256sum "$store/checklist.md" | cut -c1-64)"

  run_in "$project" doc-write-inline depot doc write notes.md --content "first line"
  expect_exit 0
  expect_out "^wrote $store/docs/notes.md\$"
  [ "$(cat "$store/docs/notes.md")" = "first line" ] || fail "docs/notes.md does not hold the inline content"

  printf 'from stdin\nsecond line\n' >"$throwaway/stdin.txt"
  stdin_file="$throwaway/stdin.txt"
  run_in "$project" doc-write-stdin depot doc write plans/piped.md --content -
  stdin_file=""
  expect_exit 0
  cmp -s "$throwaway/stdin.txt" "$store/docs/plans/piped.md" || fail "docs/plans/piped.md does not hold the piped content"

  run_in "$store" doc-write-context depot doc write context.md --content "# Context

verify-depot wrote this."
  expect_exit 0
  expect_out "^wrote $store/docs/context.md\$"
  grep -q 'verify-depot wrote this' "$store/docs/context.md" || fail "context.md was not replaced"

  run_in "$project" doc-write-escape depot doc write ../escape.md --content x
  expect_exit 1
  expect_err 'is not a document name inside the store'
  [ ! -e "$store/escape.md" ] || fail "a refused name still wrote $store/escape.md"

  [ "$(sha256sum "$store/checklist.md" | cut -c1-64)" = "$checklist_before" ] || fail "doc write changed the checklist"
  cp -R "$store/docs" "$run_dir/docs"
}

lock_pid() { sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$AGNI_HOME/run/depotd.scope.json" 2>/dev/null; }
lock_heartbeat() { sed -n 's/.*"heartbeat_millis":\([0-9]*\).*/\1/p' "$AGNI_HOME/run/depotd.scope.json" 2>/dev/null; }

start_daemon() {
  local log="$1" pid waited=0
  (cd "$project" && exec "$depotd_bin") </dev/null >"$log" 2>&1 &
  pid=$!
  daemon_pids="$daemon_pids $pid"
  note daemonPid "$pid"
  while [ "$(lock_pid)" != "$pid" ] && [ "$waited" -lt 150 ]; do
    kill -0 "$pid" 2>/dev/null || fail "depotd $pid exited during startup; read $log"
    sleep 0.1
    waited=$((waited + 1))
  done
  [ "$(lock_pid)" = "$pid" ] || fail "depotd $pid never recorded itself in run/depotd.scope.json; read $log"
  started_pid="$pid"
}

drive_daemon() {
  add_fixture_project
  run_in "$project" task-add depot task add --title "Stay held" --intent "the daemon must not start this" --role build
  expect_exit 0
  task_states >"$run_dir/task-states-before.txt"
  if grep -vqE ' (proposed)$' "$run_dir/task-states-before.txt"; then
    fail "a task is past held before the daemon starts; a daemon would launch a real worker"
  fi

  run_in "$project" depotd-no-credential "$depotd_bin"
  expect_exit 1
  expect_err 'no GitHub credential'

  printf 'verify-depot-placeholder-not-a-token\n' >"$AGNI_HOME/secrets/github-token"
  chmod 600 "$AGNI_HOME/secrets/github-token"

  start_daemon "$run_dir/depotd-1.log"
  local first="$started_pid" beat_one beat_two
  cp "$AGNI_HOME/run/depotd.scope.json" "$run_dir/depotd-lock-1.json"
  grep -q "\"projects\":\[\"$project\"\]" "$run_dir/depotd-lock-1.json" || fail "the lock record does not scope the fixture project"
  beat_one="$(lock_heartbeat)"
  sleep 2.5
  beat_two="$(lock_heartbeat)"
  note heartbeats "$beat_one $beat_two"

  run_in "$project" depotd-second "$depotd_bin"
  expect_exit 1
  expect_err 'another depot daemon already holds .*depotd\.lock'
  kill -0 "$first" 2>/dev/null || fail "the first daemon died when a second one was refused"

  run_in "$project" status-while-running depot status
  expect_out '^## Held - awaiting approval \(1\)$'
  expect_no_out 'no daemon is driving'

  stop_daemon "$first"
  kill -0 "$first" 2>/dev/null && fail "depotd $first survived SIGTERM"
  grep -q "daemon_restarted:$first:" "$run_dir/depotd-1.log" || fail "depotd $first did not journal its restart; read $run_dir/depotd-1.log"
  local polled
  polled="$(sqlite3 -readonly "$AGNI_HOME/agni.db" "select count(*) from events where kind = 'polled'")"
  [ "$polled" -gt 0 ] || fail "depotd $first never ticked; read $run_dir/depotd-1.log"

  start_daemon "$run_dir/depotd-2.log"
  local second="$started_pid"
  sleep 1.5
  stop_daemon "$second"
  grep -q "daemon_restarted:$second:" "$run_dir/depotd-2.log" || fail "the restarted daemon did not run recovery; read $run_dir/depotd-2.log"

  events >"$run_dir/events.txt"
  task_states >"$run_dir/task-states-after.txt"
  [ "$(grep -c ' | daemon_restarted | ' "$run_dir/events.txt")" = 2 ] || fail "the journal does not hold two daemon_restarted facts"
  grep -qE 'worktree_acquire|worker_turn' "$run_dir/events.txt" && fail "the daemon tried to lease or launch; read $run_dir/events.txt"
  cmp -s "$run_dir/task-states-before.txt" "$run_dir/task-states-after.txt" || fail "the daemon changed a held task"
  [ -z "$(find "$BOXR_HOME" -mindepth 1 -print -quit)" ] || fail "boxr recorded something under the throwaway BOXR_HOME"
  [ ! -e "$TREEHOUSE_ROOT" ] || [ -z "$(find "$TREEHOUSE_ROOT" -mindepth 1 -print -quit)" ] || fail "treehouse created a pool"
  [ "$beat_two" -gt "$beat_one" ] || fail "the lock heartbeat stayed at $beat_one across 2.5s of ticks; depot status will call a live daemon stale"
}

step=drive
case "$feature" in
  project-add) drive_project_add ;;
  task-lifecycle) drive_task_lifecycle ;;
  status-tui) drive_status_tui ;;
  doc-write) drive_doc_write ;;
  daemon) drive_daemon ;;
esac

step=evidence
[ -s "$run_dir/transcript.txt" ] || fail "no transcript was written"
note commands "$command_count"
make_video

step=cleanup
remove_throwaway
[ ! -e "$throwaway" ] || fail "the throwaway still exists at $throwaway"
for pid in $(sed -n 's/^daemonPid: //p' "$meta"); do
  kill -0 "$pid" 2>/dev/null && own_depotd "$pid" "$depotd_bin" && fail "depotd $pid outlived cleanup"
done
[ -s "$run_dir/transcript.txt" ] && [ -s "$meta" ] || fail "the evidence did not survive cleanup at $run_dir"

say "verify:"
say "  feature: $feature"
say "  result: ok"
say "  commands: $command_count"
say "  evidence: $run_dir"
say "  transcript: $run_dir/transcript.txt"
say "  video: $(sed -n 's/^video: //p' "$meta")"
