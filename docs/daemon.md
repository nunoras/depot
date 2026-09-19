# Running depotd as a service

One depotd serves every project in a depot store, so it runs as a single long-lived
process on the machine: a user service, not something you start per task.
The store lock at `<depot home>/depotd.lock` refuses a second instance.

## Linux: systemd user unit

Save as `~/.config/systemd/user/depotd.service`, then `systemctl --user daemon-reload`
and `systemctl --user enable --now depotd`.

```ini
[Unit]
Description=depot daemon
After=network-online.target

[Service]
ExecStart=%h/.local/bin/depotd
Restart=on-failure
RestartSec=10

[Install]
WantedBy=default.target
```

`loginctl enable-linger <user>` keeps it running when you log out.
Logs land in the journal: `journalctl --user -u depotd -f`.

## Windows: Task Scheduler

`schtasks /create /tn depotd /sc onlogon /tr "<path to depotd.exe>" /rl limited`
starts the daemon at logon; Task Scheduler restarts it only if you tick
"restart on failure" in the GUI, so for a supervised service prefer NSSM or
WinSW wrapping the same binary.

## Debugging a single project

`depotd --project <slug-or-path>` runs the same loop narrowed to one project.
It still takes the store lock, so it is a drop-in replacement for the service
while debugging, not a second daemon beside it.
