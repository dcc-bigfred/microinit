#!/usr/bin/env bash
# Retag ghcr.io/dcc-bigfred/microinit:main → :v* and :latest-release
set -euo pipefail

IMAGE="${MICROINIT_IMAGE:-ghcr.io/dcc-bigfred/microinit}"
TAG="${1:?usage: $0 <vX.Y.Z>}"

if [[ ! "${TAG}" =~ ^v ]]; then
  echo "error: tag must start with v (got ${TAG})" >&2
  exit 1
fi

echo "Retagging ${IMAGE}:main → ${IMAGE}:${TAG} and ${IMAGE}:latest-release"
docker buildx imagetools create \
  -t "${IMAGE}:${TAG}" \
  -t "${IMAGE}:latest-release" \
  "${IMAGE}:main"
