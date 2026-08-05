# microinit

<p align="center">
  <img src="docs/logo.png" alt="microinit" width="240">
</p>

Lightweight PID 1 init system and service supervisor for embedded Linux and containers.
Works in embedded systems based on Linux as well as in containers.

Lightning fast and solid-rock reliable. Inspired by supervisord and Kubernetes, handles dependencies, able to self-heal.

The services are health-checked with liveness probes and restarted. Cascade services depending on others are started as soon as dependency is started.

There are two modes - `microinit init` and `microinit supervise`. Init replaces `/sbin/init`, and supervise replaces `supervisord`.

Natively exports metrics to OpenTelemetry.

## Features

- Declarative services in `/data/etc/microinit.json` (override root with `DATA_DIR`)
- Drop-ins under `$DATA_DIR/etc/microinit.d/services/**/*.json` (lexicographic later-wins)
- inotify hot-reload of JSON config (no polling)
- `microinit supervise` — container / host supervisor (supervisord-like)
- Thread-per-service supervision with optional restart + backoff
- Unix socket IPC (`start` / `stop` / `restart` / `enable` / `disable` / `list` / `logs` / `shutdown`)
- Optional OpenTelemetry metrics (feature `otel`, on by default)
- Full PID-1 init (feature `init`, on by default): early-boot, getty, TTY attach,
  late unmount, `reboot(2)`. Android builds omit `init` (supervise-only).
- Companion `shutdown` binary — SysV-style ordered poweroff/reboot/halt via the socket
  (BusyBox `/sbin/*` fallback only when built with `init`)
- Mixed service logs on a dedicated TTY (default `/dev/tty2`); init logs on `/dev/tty3`
  (Linux `init` mode only)
- Early boot via `/etc/microinit/early-boot.sh` (or `$DATA_DIR/etc/microinit/early-boot.sh`);
  embedded portable script if neither exists (`init` feature)
- Late unmount via `/etc/microinit/unmount.sh` (or data-root override) at end of shutdown
  (`init` feature)

## Build

```bash
export RUSTUP_TOOLCHAIN=stable
make build
make release
cargo build --release                                 # otel + init (default)
cargo build --release --no-default-features --features init  # init without OTel
cargo build --release --no-default-features           # supervise-only (Android-like)
make release-musl      # aarch64-unknown-linux-musl static (microinit + shutdown)
# Android (requires ANDROID_NDK_HOME, e.g. NDK r27c) — supervise-only, no init:
make release-android                 # arm64
make release-android ARCHES="arm64 armv7 x86_64"
make test
make man
```

Android example (app-private socket path):

```bash
DATA_DIR=/data/data/com.example.app/files/microinit \
  ./microinit-android-arm64 supervise \
  --socket /data/data/com.example.app/files/microinit/microinit.sock \
  --config /data/data/com.example.app/files/microinit/etc/microinit.json \
  --console /dev/null
```

## Tests

```bash
cargo test
cargo test --no-default-features --features init   # no OTel
cargo test --no-default-features                   # supervise-only
```

## Local test harness

```bash
./examples/local-test/run.sh
```

Uses `examples/local-test/data` as `DATA_DIR`, skips early-boot, and opens two xterms for
service / init logs.

## OpenTelemetry → Grafana Alloy

Default builds include the OTLP exporter. Enable it at runtime in JSON:

```json
"openTelemetry": {
  "enable": true,
  "endpoint": "http://127.0.0.1:4318",
  "protocol": "http",
  "serviceName": "microinit",
  "exportIntervalSecs": 15
}
```

Or via `$DATA_DIR/etc/otel.env` (shared with BigFred). Process env wins over the
file; both overlay JSON. Example:

```bash
# $DATA_DIR/etc/otel.env
ENABLE_TELEMETRY=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318
OTEL_EXPORTER_OTLP_PROTOCOL=http
OTEL_SERVICE_NAME=microinit
OTEL_METRIC_EXPORT_INTERVAL=15000
```

Bare `host:4317` (BigFred gRPC style) is mapped to `http://host:4318` for the
HTTP exporter. Edits to `otel.env` are picked up on the next export cycle.

See [`grafana/alloy/microinit.alloy`](grafana/alloy/microinit.alloy) and
[`grafana/dashboards/microinit.json`](grafana/dashboards/microinit.json).

## CI / distribution

- **ORAS linux** (arm64 + early-boot + unmount + shutdown): `ghcr.io/dcc-bigfred/microinit-linux-arm64`
- **ORAS android** (supervise-only arm64 + shutdown): `ghcr.io/dcc-bigfred/microinit-android-arm64`
- **GitHub Release** also ships Android `armv7` / `x86_64` binaries (also supervise-only)
- **Container** (distroless multiarch, `supervise`): `ghcr.io/dcc-bigfred/microinit`

```bash
docker run --rm -v "$PWD/data:/data" ghcr.io/dcc-bigfred/microinit:main
# ENTRYPOINT /microinit  CMD supervise
oras pull ghcr.io/dcc-bigfred/microinit-linux-arm64:latest-release -o ./out
oras pull ghcr.io/dcc-bigfred/microinit-android-arm64:latest-release -o ./out
```

## CLI

```text
microinit [--socket PATH] init …
microinit [--socket PATH] supervise [--config PATH] [--log-to-files]
microinit start|stop|restart|enable|disable <name>
microinit start --force <name>   # ignore unmet dependsOn
microinit list
microinit logs [name] [--follow] [--lines N]
```

See `man/man8/microinit.8.mdoc` and `man/man5/microinit.json.5.mdoc`.

## License

MIT

## Documentation

See **[docs/README.md](docs/README.md)** for the full index.

- [Operator guide](docs/operator.md) — everyday CLI and boot  
- [Configuration](docs/configuration.md) — JSON, drop-ins, hot reload  
- [Service lifecycle](docs/service-lifecycle.md) — states and dependency behaviour at boot  
- [Using as supervisord](docs/using-as-supervisord.md) — PHP-FPM + NGINX example  
- [Control socket API](docs/api.md) — Unix socket protocol for integrations  
- [Architecture](docs/architecture.md) — design overview (`init` / `supervise`, reload, OTel)
