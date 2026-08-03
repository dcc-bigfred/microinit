#!/bin/sh
# early-boot.sh — portable preliminary mounts for microinit (PID 1).
# Override on device: $DATA_DIR/etc/microinit/early-boot.sh
# Env: DATA_DIR, MICROINIT_LOGS_TTY, MICROINIT_INIT_LOGS_TTY, MICROINIT_CONSOLE
#
# 1) Mount essential pseudo-filesystems (proc, sysfs, devtmpfs, …)
# 2) Apply /etc/fstab via `mount -a`
#
# Product-specific setup (data partitions, seeding, shadow bind) belongs in a
# distro overlay that replaces this script, or in the data-root override
# ($DATA_DIR/etc/microinit/early-boot.sh).
#
# Exit 0 on success. Non-zero aborts microinit boot when required.

set -eu

log() {
	echo "early-boot: $*" >&2
}

is_mounted() {
	# $1 = mountpoint path (as it appears in /proc/mounts)
	grep -q " $1 " /proc/mounts 2>/dev/null
}

mount_one() {
	# mount_one type device dir [opts]
	_type=$1
	_dev=$2
	_dir=$3
	_opts=${4:-}
	if [ -r /proc/mounts ] && is_mounted "$_dir"; then
		return 0
	fi
	if command -v mountpoint >/dev/null 2>&1; then
		mountpoint -q "$_dir" 2>/dev/null && return 0
	fi
	mkdir -p "$_dir"
	if [ -n "$_opts" ]; then
		mount -t "$_type" -o "$_opts" "$_dev" "$_dir" || return 1
	else
		mount -t "$_type" "$_dev" "$_dir" || return 1
	fi
}

# --- essential pseudo filesystems (needed before fstab) ---
if ! [ -r /proc/mounts ]; then
	mkdir -p /proc
	mount -t proc proc /proc || true
fi

mount_one sysfs sysfs /sys || true
mount_one devtmpfs devtmpfs /dev "mode=0755,nosuid" || true
mkdir -p /dev/pts
mount_one devpts devpts /dev/pts "mode=0620,gid=5" || true

# Minimal runtime mounts so init can proceed even if fstab is missing/empty
mount_one tmpfs tmpfs /run "mode=0755,nosuid,nodev" || true
mount_one tmpfs tmpfs /tmp "mode=1777,nosuid,nodev" || true

# --- fstab ---
if [ -r /etc/fstab ]; then
	log "mount -a"
	# Non-fatal: some entries may already be mounted or devices may be absent.
	mount -a || true
else
	log "no /etc/fstab; skipping mount -a"
fi

# Common dirs (may already be tmpfs from fstab)
mkdir -p /var/log /var/run /run

log "done"
exit 0
