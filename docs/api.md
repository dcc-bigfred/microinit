# Control socket API

microinit exposes a **Unix domain stream socket** for control and logs. The CLI (`microinit start`, `list`, `logs`, …) uses this protocol; you can also implement a client in any language.

Default path: **`/run/microinit.sock`** (overridable with `--socket` on both the daemon and clients, or via the `socket` field in JSON config).

---

## Framing

Each message is one **length-prefixed JSON** frame:

| Bytes | Content |
|------:|---------|
| 4 | Payload length as **little-endian** `u32` |
| N | UTF-8 JSON object |

- Maximum payload size: **16 MiB** (`16777216` bytes).  
- One request per connection is the usual pattern for short commands; `logs` with `follow: true` keeps the connection open and streams many `log` responses.  
- Concurrent client handlers are capped (32). Excess clients receive an error response.

---

## Common JSON shape

Requests and responses are tagged with a string field **`type`** (snake_case).

Unknown fields should be ignored by clients where possible; the server may add fields later.

---

## Requests

### `list`

```json
{ "type": "list" }
```

**Response:** `list` with all services.

### `status`

```json
{ "type": "status", "name": "redis" }
```

**Response:** `status` or `error` if unknown.

### `describe`

Rich status for one service: counters, uptime, direct deps, reverse deps, transitive dependency subgraph, and the last 10 lifecycle events.

```json
{ "type": "describe", "name": "nginx" }
```

**Response:** `describe` or `error` if unknown.

### `start`

```json
{ "type": "start", "name": "redis", "force": false }
```

| Field | Default | Meaning |
|-------|---------|---------|
| `name` | required | Service name |
| `force` | `false` | If `true`, start even when `dependsOn` are not satisfied |

**Response:** `ok` with an optional human-readable `message`, for example:

- `"redis: starting"`  
- `"redis: waiting for dependencies (network)"`  
- `"redis: starting with --force (unmet dependencies: network)"`  

### `stop`

```json
{ "type": "stop", "name": "redis" }
```

**Response:** `ok` or `error`.

### `restart`

```json
{ "type": "restart", "name": "redis" }
```

**Response:** `ok` or `error`.

### `enable`

Enable or disable a service and persist the override file.

```json
{ "type": "enable", "name": "dropbear", "enabled": true }
```

```json
{ "type": "enable", "name": "dropbear", "enabled": false }
```

**Response:** `ok` or `error`.  
Disabling stops the service; enabling requests a start (subject to dependencies).

### `logs`

```json
{
  "type": "logs",
  "name": "redis",
  "follow": false,
  "lines": 100
}
```

| Field | Default | Meaning |
|-------|---------|---------|
| `name` | omit / `null` | One service; omit for mixed stream |
| `follow` | required in practice | `true` = stream until the client disconnects |
| `lines` | config `logs.lines` | How many historical lines to send first |

**Response stream:**

1. Zero or more `{ "type": "log", "line": { … } }`  
2. If `follow` is `false`, a final `{ "type": "ok" }`  
3. If `follow` is `true`, further `log` frames until disconnect (no trailing `ok`)

### `shutdown`

```json
{ "type": "shutdown", "mode": "reboot" }
```

`mode` is one of: `reboot`, `poweroff`, `halt`.

**Response:** `ok`, then the daemon begins ordered shutdown (stop all services, run late-unmount script, then reboot/poweroff/halt).

Operators normally use the companion `shutdown` binary (`shutdown -r now`, …) which sends this request and falls back to BusyBox `/sbin/{poweroff,reboot,halt}` if the socket is missing.

---

## Responses

### `ok`

```json
{ "type": "ok" }
```

```json
{ "type": "ok", "message": "redis: starting" }
```

`message` is optional (omitted or null when unused).

### `error`

```json
{ "type": "error", "message": "…" }
```

### `list`

```json
{
  "type": "list",
  "services": [
    {
      "name": "redis",
      "state": "running",
      "pid": 1234,
      "restarts": 0,
      "liveness_failures": 0,
      "enabled": true
    }
  ]
}
```

`pid` may be `null` when not tracked. `liveness_failures` counts how many times `livenessProbe` failed since boot (or since the service was added on reload).

### `status`

```json
{
  "type": "status",
  "status": {
    "name": "redis",
    "state": "running",
    "pid": 1234,
    "restarts": 0,
    "liveness_failures": 0,
    "enabled": true
  }
}
```

### `describe`

```json
{
  "type": "describe",
  "describe": {
    "status": {
      "name": "nginx",
      "state": "running",
      "pid": 1240,
      "restarts": 2,
      "liveness_failures": 1,
      "enabled": true
    },
    "uptime_secs": 4320,
    "depends_on": [
      { "name": "php-fpm", "state": "running" }
    ],
    "dependents": [
      { "name": "cache", "state": "stopped" }
    ],
    "dep_nodes": [
      { "name": "cache", "state": "stopped" },
      { "name": "nginx", "state": "running" },
      { "name": "php-fpm", "state": "running" }
    ],
    "dep_edges": [
      ["php-fpm", "nginx"],
      ["nginx", "cache"]
    ],
    "events": [
      {
        "ts": "2026-08-04T18:40:01.123Z",
        "kind": "state_change",
        "from": "pending",
        "to": "starting"
      },
      {
        "ts": "2026-08-04T18:51:10.001Z",
        "kind": "liveness_failed",
        "detail": "HTTP 503"
      },
      {
        "ts": "2026-08-04T18:51:10.002Z",
        "kind": "restart"
      }
    ]
  }
}
```

| Field | Meaning |
|-------|---------|
| `depends_on` | Direct `dependsOn` (who this service needs) |
| `dependents` | Who lists this service in their `dependsOn` |
| `dep_nodes` / `dep_edges` | Transitive subgraph; edge `[A, B]` means B depends on A |
| `events` | Oldest → newest; kinds: `state_change`, `restart`, `liveness_failed` (ring, last 10) |

`uptime_secs` is omitted (or null) when the service is not currently `running`.

### `log`

```json
{
  "type": "log",
  "line": {
    "ts": "2026-08-03T18:00:00.000000000Z",
    "service": "redis",
    "level": "stdout",
    "msg": "Ready to accept connections"
  }
}
```

`level` is one of: `stdout`, `stderr`, `info`, `warn`, `error`.

---

## Service `state` values

| Value | Typical meaning |
|-------|-----------------|
| `pending` | Not started yet |
| `starting` | Start in progress |
| `running` | Daemon up |
| `succeeded` | Job finished OK |
| `failed` | Failed start or bad exit |
| `stopping` | Stop in progress |
| `stopped` | Stopped |
| `restarting` | Waiting to restart after crash |
| `disabled` | Enabled flag false |
| `waiting_for_dependency` | Blocked on `dependsOn` |

---

## Minimal client sketch (Python)

```python
import json, socket, struct

def call(path, obj):
    payload = json.dumps(obj).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.connect(path)
        s.sendall(struct.pack("<I", len(payload)) + payload)
        hdr = s.recv(4)
        (n,) = struct.unpack("<I", hdr)
        data = b""
        while len(data) < n:
            data += s.recv(n - len(data))
        return json.loads(data)

print(call("/run/microinit.sock", {"type": "list"}))
print(call("/run/microinit.sock", {"type": "start", "name": "redis", "force": False}))
```

For `logs` with `follow: true`, keep reading frames in a loop (each frame has its own 4-byte length prefix).

---

## Further reading

- [Operator guide](operator.md) — day-to-day CLI and config  
- [Architecture](architecture.md) — internals  
- [Documentation index](README.md)
