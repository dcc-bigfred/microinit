#!/usr/bin/env bash
# Publish microinit Android/arm64 OCI artifact to GHCR (ORAS).
# Intended for CI on push to main/master only.
# Usage: publish-oci-microinit-android.sh <microinit-android-arm64> [shutdown-android-arm64]
#
# Tags: main, sha-<7>
set -euo pipefail

BIN="${1:?usage: $0 <microinit-android-arm64> [shutdown-android-arm64]}"
SHUTDOWN="${2:-}"
IMAGE="${MICROINIT_OCI_ANDROID_IMAGE:-ghcr.io/dcc-bigfred/microinit-android-arm64}"
BIN_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.android.arm64.v1"
SHUTDOWN_MEDIA_TYPE="application/vnd.dcc-bigfred.shutdown.android.arm64.v1"

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

cp -f "${BIN}" "${tmpdir}/microinit-android-arm64"
chmod 755 "${tmpdir}/microinit-android-arm64"

layers=(
  "microinit-android-arm64:${BIN_MEDIA_TYPE}"
)

if [[ -n "${SHUTDOWN}" ]]; then
  if [[ ! -f "${SHUTDOWN}" ]]; then
    echo "error: shutdown binary not found: ${SHUTDOWN}" >&2
    exit 1
  fi
  cp -f "${SHUTDOWN}" "${tmpdir}/shutdown-android-arm64"
  chmod 755 "${tmpdir}/shutdown-android-arm64"
  layers+=("shutdown-android-arm64:${SHUTDOWN_MEDIA_TYPE}")
fi

annotate=(
  --annotation "org.opencontainers.image.source=${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}"
  --annotation "org.opencontainers.image.revision=${GITHUB_SHA}"
  --annotation "org.opencontainers.image.title=microinit-android"
  --annotation "org.opencontainers.image.description=microinit static binary for Android (Bionic/arm64)"
)

echo "Publishing ${IMAGE}:main and :${SHA_TAG}"
echo "  microinit: $(wc -c < "${tmpdir}/microinit-android-arm64") bytes"
if [[ -f "${tmpdir}/shutdown-android-arm64" ]]; then
  echo "  shutdown:  $(wc -c < "${tmpdir}/shutdown-android-arm64") bytes"
fi
(
  cd "${tmpdir}"
  oras push "${IMAGE}:main" "${layers[@]}" "${annotate[@]}"
  oras push "${IMAGE}:${SHA_TAG}" "${layers[@]}" "${annotate[@]}"
)
