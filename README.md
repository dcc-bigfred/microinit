<p align="center">
  <img src="docs/logo.png" alt="microinit" width="240">
</p>

# microinit

Lightweight PID 1 init system and service supervisor designed for BigFred OS.
Works in embedded systems based on Linux as well as in containers.

In **`supervise`** mode it can also act as a **supervisord replacement**: the same
process watches a declarative JSON service list, restarts daemons, and exposes
start/stop/logs over a Unix socket — without early-boot, getty, or console TTYs.

## Features

- Declarative services in `/data/etc/microinit.json` (override root with `DATA_DIR`)
- Drop-ins under `$DATA_DIR/etc/microinit.d/services/**/*.json` (lexicographic later-wins)
- inotify hot-reload of JSON config (no polling)
- `microinit supervise` — container / host supervisor (supervisord-like)
- Thread-per-service supervision with optional restart + backoff
- Unix socket IPC (`start` / `stop` / `restart` / `enable` / `disable` / `list` / `logs`)
- Optional OpenTelemetry metrics (on by default; disable with `--no-default-features`) → Grafana Alloy when `openTelemetry.enable` is true
- Mixed service logs on a dedicated TTY (default `/dev/tty2`); init logs on `/dev/tty3`
- Early boot via `/etc/microinit/early-boot.sh` (or `$DATA_DIR/etc/microinit/early-boot.sh`);
  embedded portable script if neither exists

## Build

```bash
export RUSTUP_TOOLCHAIN=stable
make build
make release
cargo build --release                        # includes OpenTelemetry exporter
cargo build --release --no-default-features  # without OpenTelemetry
make release-musl   # aarch64-unknown-linux-musl static
make test
make man
```

## Tests

```bash
cargo test
cargo test --no-default-features
```

## Local test harness

```bash
./examples/local-test/run.sh
```

Uses `examples/local-test/data` as `DATA_DIR`, skips early-boot, and opens two xterms for
service / init logs.

## OpenTelemetry → Grafana Alloy

Default builds include the OTLP exporter. Enable it at runtime in config:

```json
"openTelemetry": {
  "enable": true,
  "endpoint": "http://127.0.0.1:4318",
  "protocol": "http",
  "serviceName": "microinit",
  "exportIntervalSecs": 15
}
```

See [`grafana/alloy/microinit.alloy`](grafana/alloy/microinit.alloy) and
[`grafana/dashboards/microinit.json`](grafana/dashboards/microinit.json).

## CI / distribution

- **ORAS** (arm64 + early-boot): `ghcr.io/dcc-bigfred/microinit-linux-arm64`
- **Container** (distroless multiarch, `supervise`): `ghcr.io/dcc-bigfred/microinit`

```bash
docker run --rm -v "$PWD/data:/data" ghcr.io/dcc-bigfred/microinit:main
# ENTRYPOINT /microinit  CMD supervise
oras pull ghcr.io/dcc-bigfred/microinit-linux-arm64:latest-release -o ./out
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

- [Architecture](docs/architecture.md) — design overview, `init`/`supervise`, config, reload, OTel, distribution
