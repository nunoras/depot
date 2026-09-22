#!/bin/sh
set -eu

LOG_MAX_BYTES=${DEPOTD_LOG_MAX_BYTES:-10485760}
LOG_BACKUPS=${DEPOTD_LOG_BACKUPS:-3}
DEPOTD=${DEPOTD:-depotd}

usage() {
  cat <<'EOF'
depotd-supervisor.sh - run depotd with a rotated log

USAGE
  depotd-supervisor.sh run             supervise depotd until you stop it
  depotd-supervisor.sh rotate <log>    rotate the log once if it exceeds the cap
  depotd-supervisor.sh help            print this message

ENVIRONMENT
  AGNI_HOME               agni home; defaults to $HOME/.agni
  DEPOTD                  daemon program; defaults to depotd
  DEPOTD_LOG_MAX_BYTES    rotation cap in bytes; defaults to 10485760 (10 MiB)
  DEPOTD_LOG_BACKUPS      kept backups; defaults to 3

The log is $AGNI_HOME/run/depotd.log, rotated to depotd.log.1 through depotd.log.3.
`depot daemon restart` writes its own log to the same file and never rotates it.
EOF
}

agni_home() {
  if [ -n "${AGNI_HOME:-}" ]; then
    printf '%s' "$AGNI_HOME"
  else
    printf '%s' "$HOME/.agni"
  fi
}

rotate_file() {
  log=$1
  [ -f "$log" ] || return 0
  size=$(wc -c <"$log" | tr -d ' ')
  [ "$size" -gt "$LOG_MAX_BYTES" ] || return 0
  index=$LOG_BACKUPS
  while [ "$index" -gt 1 ]; do
    previous=$((index - 1))
    if [ -f "$log.$previous" ]; then
      mv -f "$log.$previous" "$log.$index"
    fi
    index=$previous
  done
  mv -f "$log" "$log.1"
}

run() {
  log=$(agni_home)/run/depotd.log
  mkdir -p "$(dirname "$log")"
  while :; do
    rotate_file "$log"
    "$DEPOTD" 2>&1 | while IFS= read -r line; do
      rotate_file "$log"
      printf '%s\n' "$line" >>"$log"
    done
    sleep 1
  done
}

case "${1:-run}" in
  run) run ;;
  rotate)
    log=${2:-}
    if [ -z "$log" ]; then
      usage
      exit 2
    fi
    rotate_file "$log"
    ;;
  help | --help | -h) usage ;;
  *)
    usage
    exit 2
    ;;
esac
