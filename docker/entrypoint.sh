#!/usr/bin/env sh
set -eu

APP_BIN="${APP_BIN:-/usr/local/bin/hikvision-relay}"
SDK_BASE_DEFAULT="${SDK_ROOT:-/opt/hikvision/sdk}"

HOST="${HIKVISION_HOST:-127.0.0.1}"
PORT="${HIKVISION_PORT:-8000}"
USER_NAME="${HIKVISION_USER:-admin}"
PASSWORD="${HIKVISION_PASSWORD:-}"

LISTEN_HOST="${HIKVISION_LISTEN_HOST:-0.0.0.0}"
LISTEN_PORT="${HIKVISION_LISTEN_PORT:-8554}"
LISTEN_PATH="${HIKVISION_LISTEN_PATH:-/live}"

PROBE_TIMEOUT_MS="${HIKVISION_PROBE_TIMEOUT_MS:-15000}"
STALE_AFTER_MS="${HIKVISION_STALE_AFTER_MS:-10000}"
IDLE_GRACE_MS="${HIKVISION_IDLE_GRACE_MS:-5000}"
MAX_RESTARTS="${HIKVISION_MAX_RESTARTS:-5}"

resolve_sdk_path() {
  base_path="$1"

  if [ -f "${base_path}/lib/libhcnetsdk.so" ]; then
    printf "%s\n" "${base_path}"
    return 0
  fi

  for candidate in "${base_path}"/* "${base_path}"/*/*; do
    if [ -d "${candidate}" ] && [ -f "${candidate}/lib/libhcnetsdk.so" ]; then
      printf "%s\n" "${candidate}"
      return 0
    fi
  done

  return 1
}

if [ "$#" -gt 0 ]; then
  exec "$@"
fi

set -- \
  "${APP_BIN}" \
  --host "${HOST}" \
  --port "${PORT}" \
  --user "${USER_NAME}" \
  --password "${PASSWORD}"

SDK_PATH_INPUT="${HIKVISION_SDK_PATH:-${SDK_BASE_DEFAULT}}"
if RESOLVED_SDK_PATH="$(resolve_sdk_path "${SDK_PATH_INPUT}")"; then
  export HIKVISION_SDK_PATH="${RESOLVED_SDK_PATH}"
  export LD_LIBRARY_PATH="${RESOLVED_SDK_PATH}/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
  set -- "$@" --sdk-path "${HIKVISION_SDK_PATH}"
else
  echo "HCNetSDK path resolution failed: libhcnetsdk.so not found under '${SDK_PATH_INPUT}'." >&2
  echo "Make sure your local ./sdk folder contains Linux SDK with lib/libhcnetsdk.so." >&2
  exit 1
fi

if [ "${HIKVISION_RTSP_RELAY:-true}" = "true" ]; then
  set -- "$@" \
    --rtsp-relay \
    --listen-host "${LISTEN_HOST}" \
    --listen-port "${LISTEN_PORT}" \
    --listen-path "${LISTEN_PATH}" \
    --probe-timeout-ms "${PROBE_TIMEOUT_MS}" \
    --stale-after-ms "${STALE_AFTER_MS}" \
    --idle-grace-ms "${IDLE_GRACE_MS}" \
    --max-restarts "${MAX_RESTARTS}"
fi

if [ "${HIKVISION_RESTART_ON_FAIL:-false}" = "true" ]; then
  set -- "$@" --restart-on-fail
fi

if [ "${HIKVISION_VERBOSE:-false}" = "true" ]; then
  set -- "$@" --verbose
fi

if [ "${HIKVISION_JSON_OUTPUT:-false}" = "true" ]; then
  set -- "$@" --json
fi

if [ "${HIKVISION_INCLUDE_RAW_CONFIG:-false}" = "true" ]; then
  set -- "$@" --include-raw-config
fi

exec "$@"
