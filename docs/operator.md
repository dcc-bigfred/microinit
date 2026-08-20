# Operator guide

For someone who knows basic Linux (SSH, editing files, systemd-style thinking) but **does not need to know** microinit or Rust. Day-to-day work: status, start/stop, boot.

**More detail:**

- [Configuration](configuration.md) — JSON files, drop-ins, hot reload  
- [Service lifecycle](service-lifecycle.md) — states over time, dependencies at boot  
- [Using as supervisord](using-as-supervisord.md) — PHP-FPM + NGINX in a container  

---

## What microinit does

1. Starts services from JSON config  
2. Keeps daemons alive (optional restart after crash)  
3. Lets you start/stop/enable/disable and read logs from the shell  

On embedded or custom Linux systems it often runs as **PID 1** (`/sbin/init`). In a container: **`microinit supervise`** (supervisord-like).

Settings and logs usually live under **`/data`**. Another root: **`DATA_DIR`** (absolute path only).

---

## Everyday commands

The control socket defaults to `$DATA_DIR/run/microinit.sock` (hub: `/data/run/microinit.sock`). Override with `--socket` if needed. The daemon creates the parent directory when missing.

```bash
microinit list                          # name, state, pid, restarts, enabled, live_fail
microinit list --show-labels            # same + LABELS column
microinit list -l created-by=bigfred    # filter (AND if -l repeated)
microinit info                          # version, uptime, services, OpenTelemetry
microinit info --json                   # same as JSON
microinit describe redis                # deps, reverse deps, graph, recent events
microinit start redis
microinit start --force alloy      # start even if dependsOn are not ready
microinit stop redis
microinit restart redis
microinit enable dropbear
microinit disable dropbear
microinit logs
microinit logs redis --follow
microinit logs redis --lines 100
microinit watch
microinit watch --label-key microdns-port
microinit watch -o json
```

`list` is the quickest way to see what is going on after boot. For one service — who it depends on, who depends on it, and recent restarts / liveness failures — use `describe`. `watch` reprints the list whenever name, state, or labels change (no polling). `--label-key` keeps only services that have that label key.

### Useful states

| State | Meaning |
|-------|---------|
| `running` | Daemon is up (PID shown) |
| `succeeded` | One-shot finished OK |
| `failed` | Start or exit error |
| `stopped` | Stopped on purpose |
| `disabled` | Not allowed to start |
| `waiting_for_dependency` | Waiting for `dependsOn` |
| `starting` / `restarting` | In progress |

Full example with restarts: [Service lifecycle](service-lifecycle.md).

---

## Configuration (short)

| What | Where |
|------|--------|
| Main list | `$DATA_DIR/etc/microinit.json` |
| Enable/disable override | `$DATA_DIR/etc/microinit.services.enabled-override.json` |
| Extra services | `$DATA_DIR/etc/microinit.d/services/**/*.json` |
| OpenTelemetry env | `$DATA_DIR/etc/otel.env` (optional dotenv; see README) |

Edit JSON, **save** — hot reload (no reboot in most cases). Invalid JSON is ignored.

Details: [Configuration](configuration.md).

---

## Writing a service entry

Minimal long-running service (foreground binary — preferred so microinit can track the PID):

```json
{
  "name": "myapp",
  "enabled": true,
  "daemon": true,
  "restartPolicy": "onError",
  "restartBackoff": 2,
  "startWaitSecs": 1,
  "shutdownWaitSecs": 5,
  "dependsOn": ["network"],
  "cmd": "/etc/init.d/myapp",
  "cwd": "/"
}
```

With `cmd` set to `/etc/init.d/myapp`, microinit runs:

- start → `/etc/init.d/myapp start`  
- stop → `/etc/init.d/myapp stop`  
- restart → `/etc/init.d/myapp restart`  

Or set explicit commands:

```json
"startCmd": "/usr/sbin/myapp --config /data/etc/myapp.conf",
"stopCmd": "killall myapp"
```

If `startCmd` is set, it is used instead of `cmd start`. Prefer **`exec` of the real process in the foreground** in the start script so `killall` / crashes are visible to microinit and `restartPolicy` works.

### Important fields (plain language)

| Field | Role |
|-------|------|
| `daemon` | `true` = long-lived; `false` = one-shot job |
| `restartPolicy` | `always` / `onError` (default) / `none` — auto-restart (daemons only) |
| `restartBackoff` | Seconds to wait before restarting |
| `startWaitSecs` | After start, wait this long; if the process dies in that window → `failed`. Use `1` (or more) when the start command **stays** as the service process |
| `shutdownWaitSecs` | After stop, wait then `SIGKILL` |
| `background` | At boot, start in parallel (does not wait for the console `[ OK ]` sequence as long) |
| `orderPriority` | Among ready services, lower starts earlier (default `100`; equal → name A–Z). See [Service ordering](configuration.md#service-ordering) |
| `dependsOn` | Other service names that must be `running` or `succeeded` first |
| `env` / `cwd` | Extra environment and working directory |
| `livenessProbe` | Optional periodic check. Exactly one of `cmd`, `httpUrl`, or `tcpAddr`. Shared: `interval` (default `60`), `timeout` (default `5`). `cmd` uses `successExitCodes` (default `[0]`); `httpUrl` uses `httpMethod` (default `GET`) and `httpAcceptedCodes` (default `[200]`); `tcpAddr` is `host:port`. Runs while `running` / `succeeded` / `failed`; failure re-runs start |
| `securityContext` | Optional privilege drop (`runAsUser` / `runAsGroup`) and Linux capabilities. See [Security context](#security-context). Disabled on Android |

Example one-shot with recovery (network bring-up):

```json
{
  "name": "network",
  "daemon": false,
  "cmd": "/etc/init.d/network",
  "livenessProbe": {
    "cmd": "/usr/sbin/configure-ethernet check",
    "successExitCodes": [0],
    "interval": 30,
    "timeout": 5
  }
}
```

HTTP / TCP examples:

```json
"livenessProbe": {
  "httpUrl": "http://127.0.0.1:8428/health",
  "httpMethod": "GET",
  "httpAcceptedCodes": [200],
  "interval": 30,
  "timeout": 5
}
```

```json
"livenessProbe": { "tcpAddr": "127.0.0.1:6379", "interval": 15, "timeout": 3 }
```

---

## Security context

`securityContext` (optional) drops the service to a different user/group and optionally keeps Linux capabilities across `exec`. It applies to **all** command paths (start, stop, restart, liveness `cmd` probe). microinit must run as root (or have `CAP_SETUID`+`CAP_SETGID` **and** be able to call `setgroups(2)`) to apply an identity drop; otherwise the service **fails to start** with a clear error.

**Not supported on Android** — a configured `securityContext` is rejected at config load (not silently ignored).

| Field | Role |
|-------|------|
| `runAsUser` | Login name **or** numeric uid (purely numeric string → uid) |
| `runAsGroup` | Group name **or** numeric gid; optional when the user has a passwd entry (defaults to primary gid). **Required** for numeric uids with no passwd entry |
| `capabilities` | Linux capability names (`CAP_` prefix optional). The list is **exclusive** (replaces the parent's capability set; it is not additive) |

Supplementary groups come from **`initgroups(3)`** using the passwd username
when `runAsUser` resolves to a named account (so memberships in `/etc/group`,
e.g. `dialout`, apply). Numeric uids without a passwd entry still use
`setgroups([])` (fail-closed — no inherited root groups). Environments that
deny `setgroups` (e.g. user namespaces with `/proc/self/setgroups=deny`) cannot
use `runAsUser`/`runAsGroup`.

After the drop, microinit also:
- shrinks the capability **bounding set** to the requested caps (or empty),
- sets `PR_SET_NO_NEW_PRIVS` so later `exec` cannot regain privileges via setuid binaries / file caps,
- sets `HOME` / `USER` / `LOGNAME` from passwd when known (unless overridden in `env`).

Inspect a running service with `microinit describe <name>` — it shows **Running as** (live uid/gid from `/proc`) and **Security** (the configured `securityContext`). Use `microinit describe -o json <name>` to dump the raw service object from its source file (stdout is pure JSON; the path is printed on stderr). Note: `-o json` is the **unmerged** source object; human `describe` shows the **merged** in-memory definition.

### Run a service as a non-root user

Redis as the `redis` user (group defaults to `redis`):

```json
{
  "name": "redis",
  "enabled": true,
  "daemon": true,
  "restartPolicy": "onError",
  "dependsOn": ["network"],
  "startCmd": "exec redis-server --bind 127.0.0.1 --port 6379",
  "cwd": "/var/lib/redis",
  "securityContext": {
    "runAsUser": "redis"
  },
  "livenessProbe": { "tcpAddr": "127.0.0.1:6379", "interval": 15, "timeout": 3 }
}
```

### Numeric uid/gid

`runAsUser`/`runAsGroup` accept numeric ids directly (useful in minimal images without `/etc/passwd`):

```json
"securityContext": {
  "runAsUser": "1000",
  "runAsGroup": "1000"
}
```

### Capabilities (bind a privileged port without root)

A service that needs to bind port 443 but otherwise runs unprivileged:

```json
{
  "name": "webfront",
  "enabled": true,
  "daemon": true,
  "restartPolicy": "onError",
  "startCmd": "exec /usr/bin/webfront --addr :443",
  "securityContext": {
    "runAsUser": "webfront",
    "capabilities": ["CAP_NET_BIND_SERVICE"]
  },
  "livenessProbe": { "httpUrl": "http://127.0.0.1:443/health", "httpAcceptedCodes": [200], "interval": 30, "timeout": 5 }
}
```

### Replacing image-level `setcap`

`remote-icmp` previously relied on `setcap cap_net_raw+ep` baked into the image. With `securityContext` the capability lives in the service definition and survives binary updates:

```json
{
  "name": "remote-icmp",
  "enabled": true,
  "daemon": true,
  "restartPolicy": "onError",
  "dependsOn": ["network"],
  "startCmd": "/usr/bin/bigfred-remote-icmp --config /data/etc/loco-server.conf",
  "stopCmd": "killall bigfred-remote-icmp",
  "securityContext": {
    "runAsUser": "nobody",
    "capabilities": ["CAP_NET_RAW"]
  }
}
```

> Note: `capabilities` are granted via ambient capabilities (Linux 4.3+) and are an **exclusive** set. On Android `securityContext` is rejected at config load — keep using `setcap`/root there, or omit the field from Android configs.

---

## Dependencies

Example: Redis needs the network service first.

```json
"dependsOn": ["network"]
```

If `network` is not `running` or `succeeded`, the service stays in **`waiting_for_dependency`** and starts **on its own** when ready. Manual **`stop`** cancels the wait.

```bash
microinit start --force redis   # debugging only
```

---

## Boot sequence (init mode as PID 1)

1. Kernel starts `/sbin/init` (microinit).  
2. **Early-boot** (mount `/data`, seed config, …).  
3. Config loaded from disk.  
4. Enabled services start in topological order (`dependsOn` hard edges; among ready services lower `orderPriority` first, then name). `background: true` services are started first (in that order), then foreground sequentially. Details: [Service ordering](configuration.md#service-ordering).  
5. Console `[ OK ]` / `[ FAIL ]`; getty.  
6. IPC socket; JSON files watched for reload.  

On shutdown in **`init`** mode (`shutdown -r`, IPC `shutdown`, SIGTERM, …): services stop in **reverse** of that start order, then the **unmount** script runs (unbind mounts / umount `/data`), then reboot or power-off.

In **`supervise`** mode there is no early-boot, getty, late unmount, or machine reboot — only the supervisor + socket (good for containers). On shutdown it stops services, syncs, and exits.

### Logs on a device

| Where | What |
|-------|------|
| `/dev/tty2` | Service stdout/stderr |
| `/dev/tty3` | microinit messages |
| `microinit logs …` | Same via socket |
| `$DATA_DIR/logs/` | Files when `logs.logToFiles: true` |

---

## Common tasks

**Why did it not start?**

```bash
microinit list
microinit logs nameofservice --lines 50
```

**Disable across reboots**

```bash
microinit disable grafana
```

**Apply a config change**

Edit a file under `/data/etc/`, save, wait a moment, `microinit list`.

**Service dies and stays dead**

Check `restartPolicy` and that microinit is tracking a real PID (`list` shows a PID). Scripts that background with `start-stop-daemon -b` and exit leave microinit thinking the service is fine with no PID — prefer foreground `exec`.

---

## Further reading

- [Configuration](configuration.md)  
- [Service lifecycle](service-lifecycle.md)  
- [Using as supervisord](using-as-supervisord.md)  
- [Control socket API](api.md) — scripts and UI  
- [Architecture](architecture.md) — for developers  
- [Documentation index](README.md)  
