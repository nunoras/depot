# Run and restart the depot daemon

One depot daemon runs per depot home, and it holds one lock for the whole store.
This page covers starting it, restarting it, and reading what it logs.

## Run the daemon

Start it in the foreground:

```
depotd
```

The daemon covers every registered project and writes its log records to standard error.
To drive one project only, pass `--project` with a slug.
Repeat the flag to cover several projects:

```
depotd --project first --project second
```

`--project` is a debugging flag.
While a narrowed daemon runs, no other registered project is driven, and `depot status` says so next to the affected projects.

## Restart the daemon

Stop the running daemon and start the installed one again with the same project scope:

```
depot daemon restart
```

The command stops the daemon by pid, waits for the lock to free, then starts `depotd` beside the `depot` binary.
It waits for the new daemon to record its scope, then appends the new daemon's output to `<agni home>/run/depotd.log` and prints the new pid and the scope it covers.
When the new daemon never records a fresh scope naming its pid, the command exits nonzero and says the daemon did not take the instance lock.

Stopping is a request, not a signal.
The command writes `<agni home>/run/depotd.stop` naming the pid and the daemon's start time.
A daemon honors the request only when both match, so a recycled pid is never stopped by mistake.
The daemon checks the request at the top of each tick, so the wait lasts up to one poll interval.

Set `--timeout` to bound the wait yourself:

```
depot daemon restart --timeout 120
```

The default is twice `poll_interval_seconds` plus 10 seconds.
When the bound passes, the command exits nonzero, leaves the stop request in place so a slow daemon still stops, and tells you which pid to stop by hand.
A daemon built before depot learned the stop request ignores the file, so a restart against one always reaches the bound and names the pid for you.

## Supervise the daemon with a rotated log

`scripts/depotd-supervisor.sh` runs `depotd` in a loop and keeps its log bounded.
Run it from a checkout:

```
scripts/depotd-supervisor.sh run
```

The script writes `<agni home>/run/depotd.log` and rotates it to `depotd.log.1` through `depotd.log.3`.
It rotates before the file grows past 10 MiB, and it keeps three backups.
Set `DEPOTD_LOG_MAX_BYTES` and `DEPOTD_LOG_BACKUPS` to change those limits, and set `DEPOTD` to run a different daemon program.
The script restarts the daemon one second after it exits, so stop the supervisor rather than the daemon when you want both to stay down.

Direct daemon logging is separate.
A daemon that `depot daemon restart` starts appends to the same `depotd.log` and never rotates it, so its log grows until you rotate or delete it.
Use one path or the other for a given depot home.
When the supervisor owns the daemon, stop the supervisor before you run `depot daemon restart`, because the supervisor starts the daemon again as soon as the restarted one exits.

## Check which build is running

Both binaries carry the package version and the git commit they were built from:

```
depot --version
depotd --version
```

The daemon records its build id in `<agni home>/run/depotd.scope.json`.
When the running daemon was built from a different commit than the `depot` you are running, `depot status` and `depot status --tui` print a warning naming both builds.
Restart the daemon to clear it.
A scope record from a build that predates this check carries no build id, so no warning is printed for it.
