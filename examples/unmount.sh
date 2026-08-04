#!/bin/sh
# unmount.sh — portable late unmount for microinit (PID 1) shutdown.
# Override on device: $DATA_DIR/etc/microinit/unmount.sh
# Env: DATA_DIR, MICROINIT_LOGS_TTY, MICROINIT_INIT_LOGS_TTY, MICROINIT_CONSOLE
#
# Runs after all supervised services are stopped, before reboot/poweroff/halt.
# Product images may replace this with a distro-specific script (e.g. BigFred OS
# unmounts /data and bind mounts first).
#
# Exit 0 on success. Non-zero is logged by microinit but does not block reboot.

set -eu

log() {
	echo "unmount: $*" >&2
}

is_mounted() {
	grep -q " $1 " /proc/mounts 2>/dev/null
}

sync || true

# Remount remaining filesystems read-only where possible, then unmount
# everything except essential pseudo-FS (kernel still needs them briefly).
if command -v umount >/dev/null 2>&1; then
	log "umount -a -r (skip proc,sysfs,devtmpfs,devpts,tmpfs)"
	umount -a -r -t noproc,nosysfs,nodevtmpfs,nodevpts,notmpfs 2>/dev/null || true
fi

sync || true
log "done"
exit 0
