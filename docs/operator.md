# Operator guide

This guide is for people who already know a Linux shell and need to **run and maintain services** under microinit on a device (or in a container). You do not need to know Rust.

---

## What microinit does

microinit is the program that:

1. Starts a list of services from a JSON file  
2. Keeps long-running services alive (optional restart on crash)  
3. Lets you start/stop/enable/disable services and read their logs  

On BigFred OS it is usually **PID 1** (`/sbin/init`). In a container you often run `microinit supervise` instead.

Default durable data lives under **`/data`**. You can point that elsewhere with the environment variable **`DATA_DIR`** (must be an absolute path).

---

## Everyday commands

The control socket defaults to `$DATA_DIR/run/microinit.sock` (hub: `/data/run/microinit.sock`). Override with `--socket` if needed. The daemon creates the parent directory when missing.

```bash
microinit list                          # name, state, pid, restarts, enabled, live_fail
microinit describe redis                # deps, reverse deps, graph, recent events
microinit start redis
microinit start --force alloy           # start even if dependsOn are not ready
microinit stop redis
microinit restart redis
microinit enable dropbear
microinit disable dropbear
microinit logs                          # mixed recent lines
microinit logs redis --follow
microinit logs redis --lines 100
```

`list` is the quickest way to see what is going on after boot. For one service — who it depends on, who depends on it, and recent restarts / liveness failures — use `describe`.

### Useful states

| State | Meaning |
|-------|---------|
| `running` | Daemon process is up (PID tracked) |
| `succeeded` | One-shot job finished successfully |
| `failed` | Start failed or process exited badly |
| `stopped` | Stopped on purpose |
| `disabled` | Not allowed to start (`enabled: false`) |
| `waiting_for_dependency` | Start requested; waiting for `dependsOn` |
| `starting` / `restarting` | Transition in progress |

---

## Configuration files

### Main file

**`$DATA_DIR/etc/microinit.json`** (usually `/data/etc/microinit.json`)

If the file is missing at first boot, microinit can create an empty one and an example. On BigFred OS the image often **seeds** this file from `/etc/microinit/microinit.json` during early-boot (only if `/data` does not already have a copy).

Editing `/data/etc/microinit.json` is the normal way to change what runs on a given device.

### Enable/disable override

**`$DATA_DIR/etc/microinit.services.enabled-override.json`**

Written when you run `microinit enable` / `disable`. It only stores `true`/`false` per service name and wins over the `enabled` field in the main JSON. You rarely edit this by hand.

### Drop-ins

**`$DATA_DIR/etc/microinit.d/services/**/*.json`**

Extra or overriding service definitions. Files are merged in **path sort order**; for the same service `name`, a **later** file wins. Useful for site-specific add-ons without rewriting the whole base config.

### Hot-reload

Saving any of those JSON files is picked up automatically (inotify). Invalid JSON keeps the previous config and logs a warning.

Changes to **socket path**, **log TTYs**, and **log-to-files** options need a **full microinit restart** (reboot on a hub).

---

## Writing a service entry

Minimal long-running service (foreground binary — preferred so microinit can track the PID):

```json
{
  "name": "myapp",
  "enabled": true,
  "daemon": true,
  "restart": true,
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

If `startCmd` is set, it is used instead of `cmd start`. Prefer **`exec` of the real process in the foreground** in the start script so `killall` / crashes are visible to microinit and `restart: true` works.

### Important fields (plain language)

| Field | Role |
|-------|------|
| `daemon` | `true` = long-lived; `false` = one-shot job |
| `restart` | Restart after crash (daemons only) |
| `restartBackoff` | Seconds to wait before restarting |
| `startWaitSecs` | After start, wait this long; if the process dies in that window → `failed`. Use `1` (or more) when the start command **stays** as the service process |
| `shutdownWaitSecs` | After stop, wait then `SIGKILL` |
| `background` | At boot, start in parallel (does not wait for the console `[ OK ]` sequence as long) |
| `dependsOn` | Other service names that must be `running` or `succeeded` first |
| `env` / `cwd` | Extra environment and working directory |
| `livenessProbe` | Optional periodic check. Exactly one of `cmd`, `httpUrl`, or `tcpAddr`. Shared: `interval` (default `60`), `timeout` (default `5`). `cmd` uses `successExitCodes` (default `[0]`); `httpUrl` uses `httpMethod` (default `GET`) and `httpAcceptedCodes` (default `[200]`); `tcpAddr` is `host:port`. Runs while `running` / `succeeded` / `failed`; failure re-runs start |

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

## Dependencies

Example: Redis needs the network service first.

```json
"dependsOn": ["network"]
```

What happens:

1. You (or boot) request start of `redis`.  
2. If `network` is not yet `running`/`succeeded`, redis goes to **`waiting_for_dependency`**.  
3. When `network` becomes ready, redis **starts by itself**.  
4. If you **`stop`** redis while it is waiting, that wait is cancelled. Later, when network is up, redis will **not** auto-start until you `start` it again.

To start anyway (debugging):

```bash
microinit start --force redis
```

You will see a short message on stdout, for example:

- `redis: waiting for dependencies (network)`  
- `redis: starting with --force (unmet dependencies: network)`  
- `redis: starting`  

---

## Boot sequence (init mode)

Typical hub boot:

1. Kernel starts `/sbin/init` (microinit).  
2. **Early-boot** script runs (mount `/data`, seed configs, remount root RO, …).  
3. Config is loaded from disk (**after** early-boot).  
4. Enabled services start (order respects `dependsOn`; `background: true` services start in parallel).  
5. Console shows `[ OK ]` / `[ FAIL ]` style status; getty on the console TTY.  
6. Control socket listens; config files are watched for reload.  

On shutdown (`shutdown -r`, IPC `shutdown`, SIGTERM, …): services stop in reverse dependency order, then the **unmount** script runs (unbind mounts / umount `/data`), then reboot or power-off.

In **`supervise`** mode there is no early-boot and no getty — only the supervisor + socket (good for containers). Unmount still runs at the end of shutdown if a script is present (or the embedded default).

### Logs on a device

| Where | What |
|-------|------|
| `/dev/tty2` (default) | Service stdout/stderr |
| `/dev/tty3` (default) | microinit’s own messages |
| `microinit logs …` | Same rings over the socket |
| `$DATA_DIR/logs/` | Optional files if `logs.logToFiles` is true |

---

## Common operator tasks

**See why something did not start**

```bash
microinit list
microinit logs nameofservice --lines 50
# also check /dev/tty3
```

**Temporarily disable a service across reboots**

```bash
microinit disable grafana
```

**Change config and apply**

Edit `/data/etc/microinit.json` (or a drop-in), save — wait a moment for reload — then `microinit list`.

**Service dies and stays dead**

Check `restart: true` and that microinit is tracking a real PID (`list` shows a PID). Scripts that background with `start-stop-daemon -b` and exit leave microinit thinking the service is fine with no PID — prefer foreground `exec`.

---

## Further reading

- [Control socket API](api.md) — for scripts and UIs that talk to the socket directly  
- [Architecture](architecture.md) — design background  
- [Documentation index](README.md)
