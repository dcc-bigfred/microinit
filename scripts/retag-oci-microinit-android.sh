#!/usr/bin/env bash
# Retag the microinit Android/arm64 OCI artifact from :main to a release tag and
# latest-release.
# Usage: retag-oci-microinit-android.sh <release-tag>   e.g. v0.1.0
set -euo pipefail

RELEASE_TAG="${1:?usage: $0 <release-tag>}"
IMAGE="${MICROINIT_OCI_ANDROID_IMAGE:-ghcr.io/dcc-bigfred/microinit-android-arm64}"
BIN_MEDIA_TYPE="application/vnd.dcc-bigfred.microinit.android.arm64.v1"
SHUTDOWN_MEDIA_TYPE="application/vnd.dcc-bigfred.shutdown.android.arm64.v1"
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

BIN_NAME="$(find_layer libmicroinit.so || find_layer microinit-android-arm64)" || true
if [[ -z "${BIN_NAME}" ]]; then
  echo "error: expected libmicroinit.so (or legacy microinit-android-arm64) in OCI artifact, found:" >&2
  find "${tmpdir}" -type f >&2
  exit 1
fi
# Normalize legacy layers to the jniLibs-compatible OCI layer names.
if [[ "${BIN_NAME}" != "libmicroinit.so" ]]; then
  cp -f "${tmpdir}/${BIN_NAME}" "${tmpdir}/libmicroinit.so"
  chmod 755 "${tmpdir}/libmicroinit.so"
fi

push_args=(
  "libmicroinit.so:${BIN_MEDIA_TYPE}"
)

SHUTDOWN_NAME="$(find_layer libshutdown.so || find_layer shutdown-android-arm64)" || true
if [[ -n "${SHUTDOWN_NAME}" ]]; then
  if [[ "${SHUTDOWN_NAME}" != "libshutdown.so" ]]; then
    cp -f "${tmpdir}/${SHUTDOWN_NAME}" "${tmpdir}/libshutdown.so"
    chmod 755 "${tmpdir}/libshutdown.so"
  fi
  push_args+=("libshutdown.so:${SHUTDOWN_MEDIA_TYPE}")
fi

annotate=(
  --annotation "org.opencontainers.image.source=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-dcc-bigfred/microinit}"
  --annotation "org.opencontainers.image.revision=${TAG_COMMIT}"
  --annotation "org.opencontainers.image.version=${RELEASE_TAG}"
  --annotation "org.opencontainers.image.title=microinit-android"
)

echo "Publishing ${IMAGE}:${RELEASE_TAG} and :latest-release"
echo "  libmicroinit.so: $(wc -c < "${tmpdir}/libmicroinit.so") bytes"
if [[ -n "${SHUTDOWN_NAME}" ]]; then
  echo "  libshutdown.so:  $(wc -c < "${tmpdir}/libshutdown.so") bytes"
fi
(
  cd "${tmpdir}"
  oras push "${IMAGE}:${RELEASE_TAG}" "${push_args[@]}" "${annotate[@]}"
  oras push "${IMAGE}:latest-release" "${push_args[@]}" "${annotate[@]}"
)
