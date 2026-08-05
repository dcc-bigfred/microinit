#!/usr/bin/env bash
# Cross-compile microinit + shutdown for Android (Bionic) with the NDK.
#
# Usage:
#   ANDROID_NDK_HOME=/path/to/ndk ./scripts/build-android.sh [arch...]
#
# Default arches: arm64
# Supported: arm64 | armv7 | x86_64
#
# Env:
#   ANDROID_API   — NDK API level (default 24)
#   ANDROID_NDK_HOME — required (or ANDROID_NDK_ROOT)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

API="${ANDROID_API:-24}"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "${NDK}" || ! -d "${NDK}" ]]; then
  echo "error: set ANDROID_NDK_HOME to an Android NDK (e.g. r27c)" >&2
  exit 1
fi

HOST_TAG="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
case "${HOST_TAG}" in
  linux-x86_64 | linux-aarch64 | darwin-x86_64 | darwin-arm64) ;;
  linux-arm64) HOST_TAG=linux-aarch64 ;;
  *)
    # NDK prebuilts use linux-x86_64 / darwin-x86_64 / darwin-arm64.
    if [[ "$(uname -s)" == "Linux" ]]; then
      HOST_TAG=linux-x86_64
    fi
    ;;
esac

PREBUILT="${NDK}/toolchains/llvm/prebuilt/${HOST_TAG}"
if [[ ! -d "${PREBUILT}" ]]; then
  # Fallback common CI layout
  PREBUILT="${NDK}/toolchains/llvm/prebuilt/linux-x86_64"
fi
if [[ ! -d "${PREBUILT}/bin" ]]; then
  echo "error: NDK clang not found under ${PREBUILT}/bin" >&2
  exit 1
fi
export PATH="${PREBUILT}/bin:${PATH}"

ARCHES=("$@")
if [[ ${#ARCHES[@]} -eq 0 ]]; then
  ARCHES=(arm64)
fi

mkdir -p dist

build_one() {
  local arch="$1"
  local rust_target linker triple dist_micro dist_shut

  case "${arch}" in
    arm64 | aarch64)
      rust_target=aarch64-linux-android
      triple="aarch64-linux-android${API}"
      dist_micro=microinit-android-arm64
      dist_shut=shutdown-android-arm64
      ;;
    armv7 | armeabi-v7a)
      rust_target=armv7-linux-androideabi
      triple="armv7a-linux-androideabi${API}"
      dist_micro=microinit-android-armv7
      dist_shut=shutdown-android-armv7
      ;;
    x86_64 | amd64)
      rust_target=x86_64-linux-android
      triple="x86_64-linux-android${API}"
      dist_micro=microinit-android-x86_64
      dist_shut=shutdown-android-x86_64
      ;;
    *)
      echo "error: unsupported arch '${arch}' (want arm64|armv7|x86_64)" >&2
      exit 1
      ;;
  esac

  linker="${triple}-clang"
  if ! command -v "${linker}" >/dev/null 2>&1; then
    echo "error: ${linker} not on PATH (NDK prebuilt bin)" >&2
    exit 1
  fi

  rustup target add "${rust_target}" >/dev/null

  local linker_env cc_env ar_env
  case "${rust_target}" in
    aarch64-linux-android)
      linker_env=CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER
      cc_env=CC_aarch64_linux_android
      ar_env=AR_aarch64_linux_android
      ;;
    armv7-linux-androideabi)
      linker_env=CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER
      cc_env=CC_armv7_linux_androideabi
      ar_env=AR_armv7_linux_androideabi
      ;;
    x86_64-linux-android)
      linker_env=CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER
      cc_env=CC_x86_64_linux_android
      ar_env=AR_x86_64_linux_android
      ;;
  esac

  echo "==> android ${arch} (${rust_target}, API ${API})"
  export "${linker_env}=${linker}"
  export "${cc_env}=${linker}"
  export "${ar_env}=llvm-ar"

  if [[ -n "${GITHUB_SHA:-}" ]]; then
    export MICROINIT_GIT_COMMIT="${MICROINIT_GIT_COMMIT:-${GITHUB_SHA}}"
  fi
  export MICROINIT_BUILD_TIME="${MICROINIT_BUILD_TIME:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"

  # Supervise-only: no early-boot / getty / reboot (see Cargo feature `init`).
  # Optional OTel: MICROINIT_ANDROID_OTEL=1 ./scripts/build-android.sh …
  local features=()
  if [[ "${MICROINIT_ANDROID_OTEL:-}" == "1" ]]; then
    features=(--features otel)
  fi
  cargo build --release --target "${rust_target}" --no-default-features "${features[@]}"

  cp -f "target/${rust_target}/release/microinit" "dist/${dist_micro}"
  cp -f "target/${rust_target}/release/shutdown" "dist/${dist_shut}"
  chmod 755 "dist/${dist_micro}" "dist/${dist_shut}"

  # Android jniLibs require native libraries to use the lib*.so convention.
  # Keep arch-specific release artifacts for GitHub Release compatibility.
  if [[ "${rust_target}" == "aarch64-linux-android" ]]; then
    cp -f "dist/${dist_micro}" dist/libmicroinit.so
    cp -f "dist/${dist_shut}" dist/libshutdown.so
    chmod 755 dist/libmicroinit.so dist/libshutdown.so
  fi

  file "dist/${dist_micro}" "dist/${dist_shut}" || true
  if [[ "${rust_target}" == "aarch64-linux-android" ]]; then
    file dist/libmicroinit.so dist/libshutdown.so || true
    echo "wrote dist/${dist_micro} dist/${dist_shut} dist/libmicroinit.so dist/libshutdown.so"
  else
    echo "wrote dist/${dist_micro} dist/${dist_shut}"
  fi
}

for a in "${ARCHES[@]}"; do
  build_one "${a}"
done
