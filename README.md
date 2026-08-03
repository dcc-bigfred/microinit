# microinit

Lightweight PID 1 init system and service supervisor for BigFred OS.

## Features

- Declarative services in `/data/etc/microinit.json`
  (override root with `BIGFRED_DATA_DIR` or `DATA_DIR`, same as BigFred)
- Thread-per-service supervision with optional restart + backoff
- Unix socket IPC (`start` / `stop` / `restart` / `enable` / `disable` / `list` / `logs`)
- Mixed service logs on a dedicated TTY (default `/dev/tty2`)
- microinit operational logs on a dedicated TTY (default `/dev/tty3`)
- Systemd-style `[ OK ]` / `[ FAIL ]` on the console
- Early boot via `/etc/microinit/early-boot.sh` (or data-root override); if neither
  exists, the portable script embedded in the binary runs

## Build

```bash
export RUSTUP_TOOLCHAIN=stable
make build
make release
make release-musl   # aarch64-unknown-linux-musl static
make test
make man
```

## Tests

Tests live in `tests/` against the `microinit` library (`src/lib.rs`):

```text
tests/
  config_test.rs
  console_test.rs
  early_boot_test.rs
  graph_test.rs
  ipc_test.rs
  logs_test.rs
  protocol_test.rs
  service_test.rs
  shutdown_test.rs
  signals_test.rs
  supervisor_test.rs
```

```bash
cargo test
```

## Local test harness

```bash
./examples/local-test/run.sh
```

Uses `examples/local-test/data` as `BIGFRED_DATA_DIR`, skips early-boot, and opens two
xterms (`/dev/pts/*`) for service logs and init logs. See `examples/local-test/README.md`.

## CI / distribution

GitHub Actions (`.github/workflows/`):

- **CI** — on PR and push to `main`/`master`: `fmt` + `clippy` + `test`, then static
  `aarch64-unknown-linux-musl` build. On push to `main`/`master`, publishes an ORAS
  OCI artifact to GHCR.
- **Release** — on `v*` tags: waits for CI, uploads assets to the GitHub Release, and
  retags the OCI image to `v*` and `latest-release`.

```text
ghcr.io/dcc-bigfred/microinit-linux-arm64:main
ghcr.io/dcc-bigfred/microinit-linux-arm64:sha-<7>
ghcr.io/dcc-bigfred/microinit-linux-arm64:latest-release
ghcr.io/dcc-bigfred/microinit-linux-arm64:v0.1.0
```

Pull example:

```bash
oras pull ghcr.io/dcc-bigfred/microinit-linux-arm64:latest-release -o ./out
# out/microinit-linux-arm64  out/early-boot.sh
```

## CLI

```text
microinit init [--logs-tty=/dev/tty2] [--init-logs-tty=/dev/tty3] [--console=/dev/tty1] [--no-early-boot]
microinit start|stop|restart|enable|disable <name>
microinit list
microinit logs [name] [--follow] [--lines N]
```

See `man/man8/microinit.8.mdoc` and `PLAN.md` for architecture.

## License

MIT
