#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="$ROOT_DIR/scripts/launch-host-seatd.sh"
CONTROL_BIN="${SURF_ACE_COMPOSITOR_BIN:-$ROOT_DIR/target/debug/surf-ace-compositor}"
SURF_ACE_ROOT_DEFAULT="$(cd "$ROOT_DIR/../surf-ace" 2>/dev/null && pwd || true)"
SURF_ACE_ROOT="${SURF_ACE_ROOT:-$SURF_ACE_ROOT_DEFAULT}"
ELECTRON_APP_DIR_DEFAULT="$SURF_ACE_ROOT/packages/electron"
ELECTRON_APP_DIR="${SURF_ACE_MAIN_APP_DIR:-$ELECTRON_APP_DIR_DEFAULT}"
SOCKET_PATH="/tmp/surf-ace-compositor.sock"
SURF_ACE_PORT_VALUE="${SURF_ACE_PORT:-19001}"
EVIDENCE_DIR="/tmp/surf-ace-main-runtime-verify-$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECONDS=60
KEEP_RUNNING="true"

usage() {
  cat <<'USAGE'
Usage: verify-racter-main-runtime.sh [--socket-path <path>] [--evidence-dir <dir>] [--timeout-seconds <n>] [--surf-ace-root <dir>] [--main-app-dir <dir>] [--stop-on-success]

Starts or reuses the Racter compositor+Electron main runtime only when ctl/get_status,
runtime identity, main-app attach, and capture proof all agree. Evidence is written to a
fresh timestamped directory on every run.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket-path) SOCKET_PATH="${2:-}"; shift 2 ;;
    --evidence-dir) EVIDENCE_DIR="${2:-}"; shift 2 ;;
    --timeout-seconds) TIMEOUT_SECONDS="${2:-}"; shift 2 ;;
    --surf-ace-root) SURF_ACE_ROOT="${2:-}"; shift 2 ;;
    --main-app-dir) ELECTRON_APP_DIR="${2:-}"; shift 2 ;;
    --stop-on-success) KEEP_RUNNING="false"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 2; }
}

require_cmd jq
require_cmd node
require_cmd stat

[[ -x "$LAUNCHER" ]] || { echo "missing launcher: $LAUNCHER" >&2; exit 2; }
[[ -x "$CONTROL_BIN" ]] || { echo "compositor binary missing: $CONTROL_BIN" >&2; exit 2; }
[[ -d "$ELECTRON_APP_DIR" ]] || { echo "main app dir missing: $ELECTRON_APP_DIR" >&2; exit 2; }
[[ -x "$ELECTRON_APP_DIR/node_modules/.bin/electron" ]] || { echo "electron binary missing under $ELECTRON_APP_DIR; install deps first" >&2; exit 2; }

mkdir -p "$EVIDENCE_DIR"
COMPOSITOR_LOG="$EVIDENCE_DIR/compositor.log"
CAPTURE_PATH="$EVIDENCE_DIR/capture.png"
EXPECTED_MAIN_APP_JSON="$EVIDENCE_DIR/expected_main_app_launch_intent.json"
SUMMARY_PATH="$EVIDENCE_DIR/summary.txt"
STATUS_LATEST="$EVIDENCE_DIR/status_latest.json"

MAIN_APP_LAUNCH_INTENT_JSON="$({
  jq -nc \
    --arg command "$ELECTRON_APP_DIR/node_modules/.bin/electron" \
    --arg app_dir "$ELECTRON_APP_DIR" \
    --arg socket_path "$SOCKET_PATH" \
    --arg port "$SURF_ACE_PORT_VALUE" \
    '{
      process: {
        command: $command,
        args: ["--ozone-platform=wayland", "--enable-features=UseOzonePlatform", "--no-sandbox", $app_dir],
        env: {
          SURF_ACE_COMPOSITOR_SOCKET: $socket_path,
          SURF_ACE_PORT: $port,
          SURF_ACE_WAYLAND_APP_ID: "surf-ace-main-app",
          ELECTRON_OZONE_PLATFORM_HINT: "wayland",
          ELECTRON_ENABLE_LOGGING: "1",
          XDG_RUNTIME_DIR: "/run/user/1000"
        }
      },
      binding: {
        kind: "app_id",
        app_id: "@surf-ace/electron"
      }
    }'
})"
printf '%s\n' "$MAIN_APP_LAUNCH_INTENT_JSON" > "$EXPECTED_MAIN_APP_JSON"

COMPOSITOR_PID=""
STARTED_COMPOSITOR="false"
SUCCESS="false"

cleanup() {
  local rc="$?"
  if [[ "$SUCCESS" != "true" || "$KEEP_RUNNING" != "true" ]]; then
    if [[ "$STARTED_COMPOSITOR" == "true" && -n "$COMPOSITOR_PID" ]] && kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
      kill "$COMPOSITOR_PID" >/dev/null 2>&1 || true
      wait "$COMPOSITOR_PID" >/dev/null 2>&1 || true
    fi
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

ps_for_socket() {
  ps -eo pid=,args= | awk -v sock="$SOCKET_PATH" 'index($0, " serve --runtime host ") && index($0, " --socket-path " sock) { pid=$1; $1=""; sub(/^ /, ""); print pid "|" $0 }'
}

kill_existing_socket_runtime() {
  local found="false"
  while IFS='|' read -r pid cmdline; do
    [[ -n "$pid" ]] || continue
    found="true"
    kill "$pid" >/dev/null 2>&1 || true
  done < <(ps_for_socket)
  if [[ "$found" == "true" ]]; then
    sleep 0.5
  fi
}

remove_socket_if_owned() {
  if [[ -S "$SOCKET_PATH" && -O "$SOCKET_PATH" ]]; then
    rm -f "$SOCKET_PATH"
  fi
}

status_request() {
  "$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}'
}

capture_request() {
  "$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json "{\"type\":\"capture_screen\",\"output_path\":\"$CAPTURE_PATH\"}"
}

runtime_identity_ok() {
  local status_json="$1"
  printf '%s\n' "$status_json" | jq -e --argjson expected "$MAIN_APP_LAUNCH_INTENT_JSON" '
    .ok == true and
    .status.runtime.phase == "running" and
    (.status.runtime.wayland_socket | type == "string" and length > 0) and
    .status.runtime.main_app_launch_intent == $expected and
    .status.runtime.main_app_launch_state.state == "attached" and
    (.status.runtime.main_app_launch_state.pid | type == "number") and
    (.status.runtime.main_app_surface_id != null)
  ' >/dev/null
}

runtime_status_pid_alive() {
  local status_json="$1"
  local pid
  pid="$(jq -r '.status.runtime.main_app_launch_state.pid // empty' <<<"$status_json")"
  [[ -n "$pid" && "$pid" != "null" ]] || return 1
  kill -0 "$pid" >/dev/null 2>&1
}

record_summary() {
  local status_json="$1"
  jq -r '{
    phase: .status.runtime.phase,
    wayland_socket: .status.runtime.wayland_socket,
    main_app_pid: .status.runtime.main_app_launch_state.pid,
    main_app_surface_id: .status.runtime.main_app_surface_id,
    overlay_region_count: (.status.overlay_regions.regionCount // 0),
    pane_count: (.status.panes | length)
  } | to_entries[] | "\(.key)=\(.value)"' <<<"$status_json" > "$SUMMARY_PATH"
  {
    echo "socket_path=$SOCKET_PATH"
    echo "evidence_dir=$EVIDENCE_DIR"
    echo "started_compositor=$STARTED_COMPOSITOR"
    echo "capture_path=$CAPTURE_PATH"
    [[ -n "$COMPOSITOR_PID" ]] && echo "compositor_pid=$COMPOSITOR_PID"
  } >> "$SUMMARY_PATH"
}

status_json=""
if [[ -S "$SOCKET_PATH" ]]; then
  if status_json="$(status_request 2>"$EVIDENCE_DIR/status_preexisting.stderr")"; then
    printf '%s\n' "$status_json" > "$EVIDENCE_DIR/status_preexisting.json"
    if ! runtime_identity_ok "$status_json" || ! runtime_status_pid_alive "$status_json"; then
      echo "preexisting socket failed runtime proof; replacing runtime" >&2
      kill_existing_socket_runtime
      remove_socket_if_owned
      status_json=""
    fi
  else
    echo "preexisting socket answered with control failure; replacing runtime" >&2
    kill_existing_socket_runtime
    remove_socket_if_owned
  fi
fi

if [[ -z "$status_json" ]]; then
  echo "starting compositor via seatd launcher..."
  "$LAUNCHER" --socket-path "$SOCKET_PATH" --main-app-launch-intent-json "$MAIN_APP_LAUNCH_INTENT_JSON" >"$COMPOSITOR_LOG" 2>&1 &
  COMPOSITOR_PID="$!"
  STARTED_COMPOSITOR="true"

  deadline=$((SECONDS + TIMEOUT_SECONDS))
  while (( SECONDS < deadline )); do
    if ! kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
      echo "compositor exited before proof completed; see $COMPOSITOR_LOG" >&2
      tail -n 80 "$COMPOSITOR_LOG" >&2 || true
      exit 1
    fi
    if [[ -S "$SOCKET_PATH" ]]; then
      if status_json="$(status_request 2>"$EVIDENCE_DIR/status_poll.stderr")"; then
        printf '%s\n' "$status_json" > "$STATUS_LATEST"
        if runtime_identity_ok "$status_json" && runtime_status_pid_alive "$status_json"; then
          break
        fi
      fi
    fi
    sleep 0.25
  done
fi

[[ -n "$status_json" ]] || { echo "timed out waiting for control/status proof" >&2; [[ -f "$COMPOSITOR_LOG" ]] && tail -n 80 "$COMPOSITOR_LOG" >&2 || true; exit 1; }
printf '%s\n' "$status_json" > "$STATUS_LATEST"
runtime_identity_ok "$status_json" || { echo "runtime never reached attached identity proof; see $STATUS_LATEST" >&2; exit 1; }
runtime_status_pid_alive "$status_json" || { echo "main app pid from status is not alive after proof; see $STATUS_LATEST" >&2; exit 1; }

main_app_pid="$(jq -r '.status.runtime.main_app_launch_state.pid' <<<"$status_json")"

capture_json="$(capture_request 2>"$EVIDENCE_DIR/capture.stderr")"
printf '%s\n' "$capture_json" > "$EVIDENCE_DIR/capture_response.json"
if [[ "$(jq -r '.ok // false' <<<"$capture_json")" != "true" ]]; then
  echo "capture request failed: $capture_json" >&2
  exit 1
fi
if [[ ! -f "$CAPTURE_PATH" ]]; then
  echo "capture output missing: $CAPTURE_PATH" >&2
  exit 1
fi
if [[ "$(stat -c '%s' "$CAPTURE_PATH")" -le 0 ]]; then
  echo "capture output is empty: $CAPTURE_PATH" >&2
  exit 1
fi

record_summary "$status_json"
SUCCESS="true"
echo "main runtime proof passed"
echo "summary: $SUMMARY_PATH"
