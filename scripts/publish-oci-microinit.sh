#!/usr/bin/env bash
# Publish microinit linux/arm64 OCI artifact to GHCR (ORAS).
# Intended for CI on push to main/master only.
# Usage: publish-oci-microinit.sh <microinit-linux-arm64> [early-boot.sh] [unmount.sh] [shutdown-linux-arm64]
#
# Tags: main, sha-<7>
#
# ORAS rejects absolute file paths unless --disable-path-validation is set;
# we push from a staging dir with relative basenames.
set -euo pipefail

BIN="${1:?usage: $0 <microinit-linux-arm64> [early-boot.sh] [unmount.sh] [shutdown-linux-arm64]}"
EARLY_BOOT="${2:-}"
UNMOUNT="${3:-}"
SHUTDOWN="${4:-}"
IMAGE="${MICROINIT_OCI_IMAGE:-ghcr.io/dcc-bigfred/microinit-linux-arm64}"
BIN_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.linux.arm64.v1"
EARLY_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.early-boot.sh.v1"
UNMOUNT_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.unmount.sh.v1"
SHUTDOWN_MEDIA_TYPE="application/vnd.dcc-bigfred.shutdown.linux.arm64.v1"

if [[ ! -f "${BIN}" ]]; then
  echo "error: binary not found: ${BIN}" >&2
  exit 1
fi

BRANCH="${GITHUB_REF_NAME:?GITHUB_REF_NAME required}"
if [[ "${BRANCH}" != "master" && "${BRANCH}" != "main" ]]; then
  echo "error: OCI publish is only allowed from master/main (got ${BRANCH})" >&2
  exit 1
fi

SHA_TAG="sha-${GITHUB_SHA::7}"

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "${tmpdir}"; }
trap cleanup EXIT

cp -f "${BIN}" "${tmpdir}/microinit-linux-arm64"
chmod 755 "${tmpdir}/microinit-linux-arm64"

layers=(
  "microinit-linux-arm64:${BIN_MEDIA_TYPE}"
)

if [[ -n "${EARLY_BOOT}" ]]; then
  if [[ ! -f "${EARLY_BOOT}" ]]; then
    echo "error: early-boot script not found: ${EARLY_BOOT}" >&2
    exit 1
  fi
  cp -f "${EARLY_BOOT}" "${tmpdir}/early-boot.sh"
  chmod 755 "${tmpdir}/early-boot.sh"
  layers+=("early-boot.sh:${EARLY_MEDIA_TYPE}")
fi

if [[ -n "${UNMOUNT}" ]]; then
  if [[ ! -f "${UNMOUNT}" ]]; then
    echo "error: unmount script not found: ${UNMOUNT}" >&2
    exit 1
  fi
  cp -f "${UNMOUNT}" "${tmpdir}/unmount.sh"
  chmod 755 "${tmpdir}/unmount.sh"
  layers+=("unmount.sh:${UNMOUNT_MEDIA_TYPE}")
fi

if [[ -n "${SHUTDOWN}" ]]; then
  if [[ ! -f "${SHUTDOWN}" ]]; then
    echo "error: shutdown binary not found: ${SHUTDOWN}" >&2
    exit 1
  fi
  cp -f "${SHUTDOWN}" "${tmpdir}/shutdown-linux-arm64"
  chmod 755 "${tmpdir}/shutdown-linux-arm64"
  layers+=("shutdown-linux-arm64:${SHUTDOWN_MEDIA_TYPE}")
fi

annotate=(
  --annotation "org.opencontainers.image.source=${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}"
  --annotation "org.opencontainers.image.revision=${GITHUB_SHA}"
  --annotation "org.opencontainers.image.title=microinit"
)

echo "Publishing ${IMAGE}:main and :${SHA_TAG}"
echo "  microinit: $(wc -c < "${tmpdir}/microinit-linux-arm64") bytes"
if [[ -f "${tmpdir}/shutdown-linux-arm64" ]]; then
  echo "  shutdown:  $(wc -c < "${tmpdir}/shutdown-linux-arm64") bytes"
fi
(
  cd "${tmpdir}"
  oras push "${IMAGE}:main" "${layers[@]}" "${annotate[@]}"
  oras push "${IMAGE}:${SHA_TAG}" "${layers[@]}" "${annotate[@]}"
)
