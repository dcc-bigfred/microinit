# Configuration

This page explains **where** microinit reads settings, **how** to describe a service in JSON, and **what happens when you save a file** (hot reload). No Rust required — only a text editor.

---

## Where files live

By default durable state lives under **`/data`**. Override with **`DATA_DIR`** (absolute path, e.g. `/data`).

| File or folder | Purpose |
|----------------|---------|
| `$DATA_DIR/etc/microinit.json` | Main file — service list and global options |
| `$DATA_DIR/etc/microinit.services.enabled-override.json` | Written by `microinit enable` / `disable` |
| `$DATA_DIR/etc/microinit.d/services/**/*.json` | **Drop-ins** — extra or overriding services |
| `$DATA_DIR/etc/microinit.json.example` | Example (created if missing) |

A system image may **seed** `/data/etc/microinit.json` from `/etc/microinit/microinit.json` during early-boot — only if `/data` does not already have its own copy.

With **`microinit supervise --config /path/microinit.json`**, drop-ins sit next to that file:

```text
/etc/microinit/microinit.json
/etc/microinit/microinit.d/services/
/etc/microinit/microinit.services.enabled-override.json
```

---

## Drop-ins (files in subfolders)

You do not have to put every service in one big `microinit.json`. Extra JSON under **`microinit.d/services/`** is loaded automatically.

**Rules:**

1. Any `**/*.json` in that tree (subfolders are fine).
2. Files sorted **alphabetically** (full path).
3. Same service **`name`** — the **later** file wins.
4. Add a new service or replace fields of an existing one.

**Example layout:**

```text
/data/etc/
  microinit.json
  microinit.d/
    services/
      base/network.json
      web/
        php-fpm.json
        nginx.json
```

`web/nginx.json` can set `"dependsOn": ["php-fpm"]` while `php-fpm` lives in `web/php-fpm.json`. Folder order matters only when the **same** service name appears twice.

**Tip:** folder names like `10-base/`, `20-web/` control override order.

---

## Main file (skeleton)

```json
{
  "version": 1,
  "socket": "/run/microinit.sock",
  "console": "/dev/tty1",
  "logs": {
    "tty": "/dev/tty2",
    "initTty": "/dev/tty3",
    "lines": 300,
    "logToFiles": false,
    "dir": "/data/logs"
  },
  "services": []
}
```

| Field | Meaning |
|-------|---------|
| `version` | Schema version (use `1`) |
| `socket` | Control socket for `microinit list`, UI, scripts |
| `console` | Boot console and getty (init mode) |
| `logs.tty` | Service logs (init mode) |
| `logs.initTty` | microinit’s own messages |
| `logs.logToFiles` | If `true`, also files under `$DATA_DIR/logs/` |
| `openTelemetry` | Optional metrics (see README); also `$DATA_DIR/etc/otel.env` |

Most operators only edit **`services`**.

---

## One service entry

### Long-running service (daemon)

```json
{
  "name": "myapp",
  "enabled": true,
  "daemon": true,
  "restart": true,
  "restartBackoff": 2,
  "startWaitSecs": 1,
  "shutdownWaitSecs": 5,
  "orderPriority": 100,
  "dependsOn": ["network"],
  "cmd": "/etc/init.d/myapp",
  "cwd": "/"
}
```

With `cmd`, microinit runs `cmd start`, `cmd stop`, `cmd restart`.

Or explicit commands:

```json
"startCmd": "/usr/sbin/myapp --config /data/etc/myapp.conf",
"stopCmd": "killall myapp"
```

**Important:** start should **`exec`** the program in the **foreground** so microinit can track the process and restart on crash. A script that backgrounds a daemon and exits makes microinit think everything is fine with no PID tracked.

### One-shot job (at boot)

```json
{
  "name": "network",
  "daemon": false,
  "restart": false,
  "orderPriority": 30,
  "cmd": "/etc/init.d/network"
}
```

Success → `succeeded`. Failure → `failed`.

### Fields — short reference

| Field | Role |
|-------|------|
| `name` | Unique name (CLI, `dependsOn`) |
| `enabled` | `false` = do not start at boot |
| `daemon` | `true` = long-lived; `false` = one-shot |
| `restart` | Retry after crash (daemon only) |
| `restartBackoff` | Seconds before each retry |
| `startWaitSecs` | After start, wait; if process dies in window → `failed` |
| `shutdownWaitSecs` | After stop, wait then `SIGKILL` |
| `background` | Parallel start at boot |
| `orderPriority` | Among ready services, lower starts earlier (default `100`; equal → alphabetical name) |
| `dependsOn` | These must be `running` or `succeeded` first |
| `livenessProbe` | Optional health check; failure triggers restart |

### Liveness probe

Exactly **one** of `cmd`, `httpUrl`, or `tcpAddr`:

```json
"livenessProbe": {
  "httpUrl": "http://127.0.0.1:8080/health",
  "httpAcceptedCodes": [200],
  "interval": 30,
  "timeout": 5
}
```

Defaults: `interval` 60 s, `timeout` 5 s.

---

## Enable / disable without editing JSON

```bash
microinit disable grafana
microinit enable grafana
```

The override file wins over `"enabled"` in JSON.

---

## Hot reload (save file → apply)

microinit watches JSON via **inotify** (no periodic disk scanning). On save:

1. Short pause (~300 ms) — one save often emits several events.
2. Load and merge (main + drop-ins + override).
3. **Invalid JSON** → old config kept; warning in logs.
4. **Valid JSON** → diff services:
   - new → start (with `dependsOn`)
   - removed → stop
   - definition changed → restart
   - `enabled` toggled → start or stop

**Reboot is usually not needed.** After saving:

```bash
microinit list
```

### What does **not** hot-reload

Requires **microinit restart** (on PID 1 hosts: reboot):

- `socket` path
- `socketAllowUsers` — optional list of login names allowed to connect to the
  control socket in addition to the daemon uid (resolved via passwd at load;
  unknown names abort config). When non-empty, the socket is `0660` owned by
  `daemon_uid:<group of the first name>` (prefer a group matching the login,
  else that user's primary gid — typically `bigfred:bigfred` on the hub).
  **Order matters:** later names are allowed by uid peer-check only; they must
  still be able to open a `0660` socket for that group (put the intended
  socket-group owner first).
- `logs.*` (TTYs, `logToFiles`, buffer size)
- `console`

---

## Service ordering

Boot and shutdown order come from a topological sort of `dependsOn`, with
`orderPriority` as the tie-breaker among services that are currently ready.

**Rules (in order):**

1. `dependsOn` builds a hard DAG — a service cannot start before its
   dependencies.
2. Among services with all dependencies satisfied (ready), pick the lowest
   `orderPriority` first.
3. Equal `orderPriority` → alphabetical `name`.
4. That list is split into foreground / background for boot parallelism.
5. Shutdown uses the **reverse** of the start order.

Default when the field is omitted: **`100`**.

Do **not** confuse this with drop-in **merge** order (files sorted by path
alphabetically; later file wins for the same service name) — that only decides
which definition is kept, not boot order.

### Example 1 — priority only (no deps)

```json
[
  { "name": "cron", "orderPriority": 50 },
  { "name": "sysctl", "orderPriority": 10 },
  { "name": "watchdog", "orderPriority": 20 }
]
```

Start order: `sysctl` → `watchdog` → `cron`.

### Example 2 — same priority → alphabetical

```json
[
  { "name": "redis", "orderPriority": 100 },
  { "name": "alloy", "orderPriority": 100 },
  { "name": "microdns", "orderPriority": 100 }
]
```

Start order: `alloy` → `microdns` → `redis`.

### Example 3 — `dependsOn` blocks; then priority among ready

```json
[
  { "name": "network", "orderPriority": 30 },
  { "name": "app", "orderPriority": 10, "dependsOn": ["network"] },
  { "name": "cron", "orderPriority": 50 }
]
```

1. Ready at start: `network` (30), `cron` (50) → start **`network`**.
2. After `network`: ready `app` (10) and `cron` (50) → **`app`**, then **`cron`**.

Start order: `network` → `app` → `cron`.  
(`app` has a lower priority than `cron`, but cannot overtake `network`.)

### Example 4 — shutdown = reverse

For example 3: stop `cron` → `app` → `network`.

### Example 5 — mini hub

| name | orderPriority | dependsOn |
|------|---------------|-----------|
| sysctl | 10 | — |
| network | 30 | — |
| redis | 100 | network |
| bigfred | 300 | network, redis |
| grafana | 400 | — |

Start: `sysctl` → `network` → `redis` → `bigfred` → `grafana`.  
Shutdown: `grafana` → `bigfred` → `redis` → `network` → `sysctl`.

(On a real hub image, `grafana` also `dependsOn` `victoriametrics`; the table
above is simplified.)

### Example 6 — `background` vs `orderPriority`

`orderPriority` only orders the topological list. At boot, microinit still
starts **all** `background: true` services first (fire-and-forget, in topo
order), then foreground services sequentially. A low `orderPriority` on a
foreground service does **not** make it start before background peers.

See also [Operator guide](operator.md) (boot sequence) and
[Architecture](architecture.md).

---

## Dependencies

```json
"dependsOn": ["network", "redis"]
```

A service starts when every listed name is **`running`** or **`succeeded`**. Until then: **`waiting_for_dependency`** — it starts **on its own** when ready.

`microinit stop` while waiting **cancels** the wait — fixing the dependency does **not** start the service without `microinit start`.

```bash
microinit start --force myapp   # debugging only
```

Boot example with restarts: [Service lifecycle](service-lifecycle.md).
Ordering of who gets `Start` first: [Service ordering](#service-ordering).

---

## Further reading

- [Operator guide](operator.md) — everyday commands  
- [Using as supervisord](using-as-supervisord.md) — PHP-FPM + NGINX  
- [Service lifecycle](service-lifecycle.md)  
- `man/man5/microinit.json.5.mdoc` — full field list  
