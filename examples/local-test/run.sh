#!/usr/bin/env bash
# Local harness: run microinit against examples/local-test/data with two xterm PTS
# windows for service logs and init logs.
#
# Usage:
#   ./examples/local-test/run.sh
#   MICROINIT_BIN=/path/to/microinit ./examples/local-test/run.sh
#
# Requires: xterm (or XTERM_CMD), a writable display, and a built microinit binary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$ROOT/../.." && pwd)"
DATA="$ROOT/data"
TEMPLATE="$ROOT/microinit.json.template"
XTERM_CMD="${XTERM_CMD:-xterm}"

mkdir -p "$DATA/etc" "$DATA/logs" "$DATA/run"

# --- resolve microinit binary ---
resolve_bin() {
	if [[ -n "${MICROINIT_BIN:-}" && -x "${MICROINIT_BIN}" ]]; then
		echo "${MICROINIT_BIN}"
		return
	fi
	if [[ -x "$REPO_ROOT/target/debug/microinit" ]]; then
		echo "$REPO_ROOT/target/debug/microinit"
		return
	fi
	if [[ -x "$REPO_ROOT/target/release/microinit" ]]; then
		echo "$REPO_ROOT/target/release/microinit"
		return
	fi
	echo "building microinit (debug)…" >&2
	(cd "$REPO_ROOT" && RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" cargo build)
	echo "$REPO_ROOT/target/debug/microinit"
}

MICROINIT="$(resolve_bin)"

# --- materialize config with absolute data paths ---
sed "s|@DATA@|${DATA}|g" "$TEMPLATE" >"$DATA/etc/microinit.json"

# --- open an xterm and capture its slave PTS ---
# The slave must stay open (sleep infinity) so microinit can write to it.
open_pts_window() {
	local title=$1
	local pts_file=$2
	local marker
	marker="$(mktemp)"
	rm -f "$pts_file"
	: >"$marker"

	if ! command -v "$XTERM_CMD" >/dev/null 2>&1; then
		echo "error: '$XTERM_CMD' not found; install xterm or set XTERM_CMD" >&2
		exit 1
	fi

	"$XTERM_CMD" \
		-T "$title" \
		-geometry 100x28 \
		-e sh -c "
			tty >'$pts_file'
			rm -f '$marker'
			printf '\\033]0;%s\\007' '$title'
			clear
			echo \"=== $title ===\"
			echo \"pts=\$(cat '$pts_file')\"
			echo
			exec sleep infinity
		" &
	echo $! >"${pts_file}.pid"

	local i=0
	while [[ -e "$marker" || ! -s "$pts_file" ]]; do
		i=$((i + 1))
		if [[ $i -gt 100 ]]; then
			echo "error: timed out waiting for PTS from xterm ($title)" >&2
			exit 1
		fi
		sleep 0.05
	done
	rm -f "$marker"
}

cleanup() {
	local code=$?
	set +e
	if [[ -n "${MICROINIT_PID:-}" ]]; then
		kill -TERM "$MICROINIT_PID" 2>/dev/null
		wait "$MICROINIT_PID" 2>/dev/null
	fi
	for f in "${SERVICE_PTS_FILE:-}" "${INIT_PTS_FILE:-}"; do
		[[ -n "$f" && -f "${f}.pid" ]] || continue
		kill "$(cat "${f}.pid")" 2>/dev/null
		rm -f "${f}.pid" "$f"
	done
	rm -f "$DATA/run/microinit.sock"
	exit "$code"
}
trap cleanup EXIT INT TERM

SERVICE_PTS_FILE="$(mktemp)"
INIT_PTS_FILE="$(mktemp)"
rm -f "$SERVICE_PTS_FILE" "$INIT_PTS_FILE"

echo "opening log windows…"
open_pts_window "microinit · service logs" "$SERVICE_PTS_FILE"
open_pts_window "microinit · init logs" "$INIT_PTS_FILE"

SERVICE_PTS="$(cat "$SERVICE_PTS_FILE")"
INIT_PTS="$(cat "$INIT_PTS_FILE")"
CONSOLE_TTY="$(tty 2>/dev/null || true)"
if [[ -z "$CONSOLE_TTY" || ! -e "$CONSOLE_TTY" ]]; then
	CONSOLE_TTY="/dev/null"
fi

echo "BIGFRED_DATA_DIR=$DATA"
echo "service logs → $SERVICE_PTS"
echo "init logs    → $INIT_PTS"
echo "console      → $CONSOLE_TTY"
echo "socket       → $DATA/run/microinit.sock"
echo
echo "Ctrl-C stops microinit and closes the xterms."
echo

export BIGFRED_DATA_DIR="$DATA"
# Keep socket path aligned with generated config for CLI subcommands.
export MICROINIT_SOCKET="$DATA/run/microinit.sock"

"$MICROINIT" \
	--socket "$DATA/run/microinit.sock" \
	init \
	--no-early-boot \
	--config "$DATA/etc/microinit.json" \
	--logs-tty "$SERVICE_PTS" \
	--init-logs-tty "$INIT_PTS" \
	--console "$CONSOLE_TTY" &
MICROINIT_PID=$!

wait "$MICROINIT_PID"
