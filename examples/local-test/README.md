# Local microinit harness
#
# Layout (created/used by run.sh):
#
#   data/
#     etc/microinit.json   # generated from microinit.json.template
#     logs/                # per-service log files
#     run/microinit.sock   # IPC socket
#
# Services in the template:
#   hello   — one-shot job (`echo`)
#   sleeper — daemon (`sleep` loop with ticks on stdout)
#
# run.sh opens two xterms, captures their /dev/pts/* paths, and passes them as
# --logs-tty / --init-logs-tty. Boot progress is teed to the starting terminal and
# the init-logs xterm; after boot, lifecycle (start/stop/…) is init-logs only.
# Getty is skipped automatically when not PID 1.
#
#   ./examples/local-test/run.sh
#
# Optional env:
#   MICROINIT_BIN  path to microinit binary
#   XTERM_CMD      terminal emulator (default: xterm)
