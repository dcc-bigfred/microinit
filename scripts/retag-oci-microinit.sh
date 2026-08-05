#!/usr/bin/env bash
# Retag the microinit linux/arm64 OCI artifact from :main to a release tag and
# latest-release.
# Usage: retag-oci-microinit.sh <release-tag>   e.g. v0.1.0
#
# Requires: GITHUB_SHA (tag commit), oras
#
# ORAS rejects absolute file paths; we push from the pull dir with relative names.
set -euo pipefail

RELEASE_TAG="${1:?usage: $0 <release-tag>}"
IMAGE="${MICROINIT_OCI_IMAGE:-ghcr.io/dcc-bigfred/microinit-linux-arm64}"
BIN_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.linux.arm64.v1"
EARLY_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.early-boot.sh.v1"
UNMOUNT_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.unmount.sh.v1"
SHUTDOWN_MEDIA_TYPE="application/vnd.dcc-bigfred.shutdown.linux.arm64.v1"
TAG_COMMIT="${GITHUB_SHA:?GITHUB_SHA required (tag commit)}"

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "${tmpdir}"; }
trap cleanup EXIT

echo "Pulling ${IMAGE}:main…"
oras pull "${IMAGE}:main" -o "${tmpdir}"

find_layer() {
  local want="$1"
  if [[ -f "${tmpdir}/${want}" ]]; then
    echo "${want}"
    return 0
  fi
  mapfile -t files < <(find "${tmpdir}" -type f \
    ! -name 'manifest.json' ! -name 'config.json' \
    -name "${want}" -printf '%f\n')
  if [[ ${#files[@]} -eq 1 ]]; then
    echo "${files[0]}"
    return 0
  fi
  return 1
}

BIN_NAME="$(find_layer microinit-linux-arm64)" || true
if [[ -z "${BIN_NAME}" ]]; then
  echo "error: expected microinit-linux-arm64 in OCI artifact, found:" >&2
  find "${tmpdir}" -type f >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TAG_COMMIT_SHORT="${TAG_COMMIT:0:7}"
"${SCRIPT_DIR}/inject-elf-version.sh" "${tmpdir}/${BIN_NAME}" "${RELEASE_TAG}" "${TAG_COMMIT_SHORT}"

push_args=(
  "${BIN_NAME}:${BIN_MEDIA_TYPE}"
)

EARLY_NAME="$(find_layer early-boot.sh)" || true
if [[ -n "${EARLY_NAME}" ]]; then
  push_args+=("${EARLY_NAME}:${EARLY_MEDIA_TYPE}")
fi

UNMOUNT_NAME="$(find_layer unmount.sh)" || true
if [[ -n "${UNMOUNT_NAME}" ]]; then
  push_args+=("${UNMOUNT_NAME}:${UNMOUNT_MEDIA_TYPE}")
fi

SHUTDOWN_NAME="$(find_layer shutdown-linux-arm64)" || true
if [[ -n "${SHUTDOWN_NAME}" ]]; then
  "${SCRIPT_DIR}/inject-elf-version.sh" "${tmpdir}/${SHUTDOWN_NAME}" "${RELEASE_TAG}" "${TAG_COMMIT_SHORT}"
  push_args+=("${SHUTDOWN_NAME}:${SHUTDOWN_MEDIA_TYPE}")
fi

annotate=(
  --annotation "org.opencontainers.image.source=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-dcc-bigfred/microinit}"
  --annotation "org.opencontainers.image.revision=${TAG_COMMIT}"
  --annotation "org.opencontainers.image.version=${RELEASE_TAG}"
  --annotation "org.opencontainers.image.title=microinit"
)

echo "Publishing ${IMAGE}:${RELEASE_TAG} and :latest-release"
echo "  microinit: $(wc -c < "${tmpdir}/${BIN_NAME}") bytes"
if [[ -n "${SHUTDOWN_NAME}" ]]; then
  echo "  shutdown:  $(wc -c < "${tmpdir}/${SHUTDOWN_NAME}") bytes"
fi
(
  cd "${tmpdir}"
  oras push "${IMAGE}:${RELEASE_TAG}" "${push_args[@]}" "${annotate[@]}"
  oras push "${IMAGE}:latest-release" "${push_args[@]}" "${annotate[@]}"
)
