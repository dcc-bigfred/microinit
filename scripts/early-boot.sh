#!/bin/sh
# early-boot.sh — portable preliminary mounts for microinit (PID 1).
# Override on device: $DATA_DIR/etc/microinit/early-boot.sh
# Env: DATA_DIR, MICROINIT_LOGS_TTY, MICROINIT_INIT_LOGS_TTY, MICROINIT_CONSOLE
#
# 1) Mount essential pseudo-filesystems (proc, sysfs, devtmpfs, …)
# 2) fsck -y every real block filesystem that is not yet mounted
# 3) Apply /etc/fstab via `mount -a`
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

# True if $1 (block device path or symlink) is a source in /proc/mounts.
dev_is_mounted() {
	_want=$1
	_real=$(readlink -f "$_want" 2>/dev/null || echo "$_want")
	[ -r /proc/mounts ] || return 1
	while read -r _src _mp _fstype _opts _rest; do
		case "$_src" in
		\#* | '') continue ;;
		esac
		_sreal=$(readlink -f "$_src" 2>/dev/null || echo "$_src")
		if [ "$_src" = "$_want" ] || [ "$_src" = "$_real" ] || \
			[ "$_sreal" = "$_real" ] || [ "$_sreal" = "$_want" ]; then
			return 0
		fi
	done < /proc/mounts
	return 1
}

# Auto-repair one block device. Never aborts boot (fsck exit codes are noisy).
# Skips missing/non-block devices, unknown/empty TYPE, and already-mounted sources.
fsck_one() {
	_dev=$1
	[ -n "$_dev" ] || return 0
	# Resolve LABEL=/UUID= if findfs exists.
	case "$_dev" in
	LABEL=* | UUID=* | PARTUUID=* | PARTLABEL=*)
		if command -v findfs >/dev/null 2>&1; then
			_resolved=$(findfs "$_dev" 2>/dev/null || true)
			[ -n "${_resolved:-}" ] && _dev=$_resolved
		fi
		;;
	esac
	[ -b "$_dev" ] || return 0

	if dev_is_mounted "$_dev"; then
		log "fsck skip $_dev (already mounted)"
		return 0
	fi

	_fstype=
	if command -v blkid >/dev/null 2>&1; then
		_fstype=$(blkid -o value -s TYPE "$_dev" 2>/dev/null || true)
	fi
	case "$_fstype" in
	'' | swap | crypto_LUKS)
		# Unformatted / swap / crypto — nothing useful to fsck yet.
		return 0
		;;
	esac

	_rc=0
	if command -v fsck >/dev/null 2>&1; then
		log "fsck -y $_dev (TYPE=${_fstype:-unknown})"
		fsck -y -T "$_dev" || _rc=$?
	elif command -v e2fsck >/dev/null 2>&1; then
		case "$_fstype" in
		ext2 | ext3 | ext4)
			log "e2fsck -y $_dev"
			e2fsck -y "$_dev" || _rc=$?
			;;
		*)
			log "fsck skip $_dev (no fsck binary for TYPE=$_fstype)"
			return 0
			;;
		esac
	else
		log "fsck skip $_dev (no fsck/e2fsck on PATH)"
		return 0
	fi

	# 0 = clean, 1 = errors corrected. Higher bits: see fsck(8).
	case $_rc in
	0) ;;
	1) log "fsck $_dev: errors corrected" ;;
	*) log "WARNING: fsck $_dev exited $_rc (continuing boot)" ;;
	esac
	return 0
}

# Remount root RO so it can be checked, then fsck every real fstab device
# (and any leftover with a positive pass number via fsck -A when available).
fsck_before_mount() {
	# Kernel already mounted / — remount RO so e2fsck/fsck will accept it.
	_rootdev=
	if is_mounted /; then
		log "remount,ro / for fsck"
		mount -o remount,ro / 2>/dev/null || true
		_rootdev=$(awk '$2 == "/" { print $1; exit }' /proc/mounts 2>/dev/null || true)
	fi

	if [ -r /etc/fstab ]; then
		if command -v fsck >/dev/null 2>&1; then
			_rc=0
			log "fsck -A -y (fstab pass numbers)"
			fsck -A -y -T || _rc=$?
			case $_rc in
			0) ;;
			1) log "fsck -A: errors corrected" ;;
			*) log "WARNING: fsck -A exited $_rc (continuing boot)" ;;
			esac
		fi
		while read -r _fsck_dev _fsck_mp _fsck_type _fsck_opts _fsck_dump _fsck_pass _rest; do
			case "$_fsck_dev" in
			'' | \#*) continue ;;
			esac
			case "$_fsck_type" in
			proc | sysfs | devtmpfs | devpts | tmpfs | ramfs | cgroup* | \
			overlay | squashfs | nfs* | cifs | autofs | debugfs | \
			securityfs | pstore | bpf | tracefs | hugetlbfs | mqueue | \
			configfs | fusectl | swap)
				continue
				;;
			esac
			fsck_one "$_fsck_dev"
		done < /etc/fstab
	fi

	# Explicit root check: fsck_one skips mounted devices, and BusyBox
	# fsck -A may be a stub. Root is RO now, so force-repair is safe.
	if [ -n "${_rootdev:-}" ]; then
		_rc=0
		if command -v fsck >/dev/null 2>&1; then
			log "fsck -y $_rootdev (root, remounted ro)"
			fsck -y -T "$_rootdev" || _rc=$?
		elif command -v e2fsck >/dev/null 2>&1; then
			log "e2fsck -y $_rootdev (root, remounted ro)"
			e2fsck -y "$_rootdev" || _rc=$?
		else
			return 0
		fi
		case $_rc in
		0) ;;
		1) log "fsck $_rootdev: errors corrected" ;;
		*) log "WARNING: fsck $_rootdev exited $_rc (continuing boot)" ;;
		esac
	fi
}

# --- essential pseudo filesystems (needed before fstab / fsck) ---
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

# --- fsck real filesystems before any block mount ---
fsck_before_mount

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
