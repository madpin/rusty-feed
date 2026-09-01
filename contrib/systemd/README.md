# systemd units

The default `rusty-feed.service` runs the HTTP process with its embedded worker.
Install the binary, create a locked `rusty-feed` system account, put configuration
in `/etc/rusty-feed/rusty-feed.env`, copy the unit to `/etc/systemd/system`, and run:

```console
sudo systemctl daemon-reload
sudo systemctl enable --now rusty-feed.service
systemctl status rusty-feed.service
```

For independently scaled workers, set `RUN_EMBEDDED_WORKER=false` in the shared
environment file, install `rusty-feed-worker.service`, and enable both units. Never
enable the standalone worker while leaving the embedded worker enabled unless
the extra queue concurrency is deliberate.

Both binaries auto-detect systemd's notify socket, report readiness and graceful
shutdown, and ping the configured watchdog at half its interval. Set
`SYSTEMD_NOTIFY_ENABLED=false` only when a supervisor supplies incompatible
notification environment variables. Adjust `WatchdogSec`, restart policy, and
hardening for local requirements; keep the state/model paths under
`/var/lib/rusty-feed` or explicitly add writable paths.
