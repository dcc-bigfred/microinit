#!/usr/bin/env bash
# Inject release version metadata into an ELF binary as section
# .microinit.version (JSON {"version":"v1.2.3","commit":"abc1234"}).
# Usage: inject-elf-version.sh <binary> <version> <tag-commit>
set -euo pipefail

BINARY="${1:?usage: $0 <binary> <version> <tag-commit>}"
VERSION="${2:?}"
TAG_COMMIT="${3:?}"

if [[ ! -f "${BINARY}" ]]; then
  echo "error: binary not found: ${BINARY}" >&2
  exit 1
fi

if ! command -v objcopy >/dev/null 2>&1; then
  echo "error: objcopy not found (install binutils)" >&2
  exit 1
fi

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "${tmpdir}"; }
trap cleanup EXIT

section_file="${tmpdir}/microinit.version.json"
printf '{"version":"%s","commit":"%s"}' "${VERSION}" "${TAG_COMMIT}" > "${section_file}"

objcopy --remove-section .microinit.version "${BINARY}" 2>/dev/null || true
objcopy --add-section ".microinit.version=${section_file}" "${BINARY}"

echo "Injected .microinit.version into ${BINARY}: version=${VERSION} commit=${TAG_COMMIT}"
