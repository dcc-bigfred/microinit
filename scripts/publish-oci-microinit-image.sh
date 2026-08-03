#!/usr/bin/env bash
# Build and push multiarch distroless image ghcr.io/dcc-bigfred/microinit.
# Requires: dist/microinit-linux-amd64, dist/microinit-linux-arm64, docker buildx.
# Tags: main, sha-<7>
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

AMD64="${1:-dist/microinit-linux-amd64}"
ARM64="${2:-dist/microinit-linux-arm64}"
IMAGE="${MICROINIT_IMAGE:-ghcr.io/dcc-bigfred/microinit}"

for f in "${AMD64}" "${ARM64}"; do
  if [[ ! -f "${f}" ]]; then
    echo "error: missing binary: ${f}" >&2
    exit 1
  fi
done

# Dockerfile expects dist/microinit-linux-${TARGETARCH}
mkdir -p dist
cp -f "${AMD64}" dist/microinit-linux-amd64
cp -f "${ARM64}" dist/microinit-linux-arm64
chmod 755 dist/microinit-linux-amd64 dist/microinit-linux-arm64

BRANCH="${GITHUB_REF_NAME:?GITHUB_REF_NAME required}"
if [[ "${BRANCH}" != "master" && "${BRANCH}" != "main" ]]; then
  echo "error: image publish is only allowed from master/main (got ${BRANCH})" >&2
  exit 1
fi

SHA_TAG="sha-${GITHUB_SHA::7}"

echo "Publishing ${IMAGE}:main and :${SHA_TAG}"
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -f docker/Dockerfile \
  -t "${IMAGE}:main" \
  -t "${IMAGE}:${SHA_TAG}" \
  --push \
  .
