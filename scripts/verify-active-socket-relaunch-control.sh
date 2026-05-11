#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTROL_BIN="${SURF_ACE_COMPOSITOR_BIN:-$ROOT_DIR/target/debug/surf-ace-compositor}"
SOCKET_PATH="${SURF_ACE_SOCKET:-/tmp/surf-ace-compositor.sock}"
EVIDENCE_DIR="/tmp/surf-ace-relaunch-control-verify-$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECONDS=30
LAUNCH_COMMAND="${SURF_ACE_RELAUNCH_COMMAND:-foot --app-id surf-ace-main-app sh -lc top}"

usage() {
  cat <<'USAGE'
Usage: verify-active-socket-relaunch-control.sh [--socket-path <path>] [--evidence-dir <dir>] [--timeout-seconds <n>] [--launch <command>]

Verifies that an already-running compositor accepts `serve --launch ... --socket-path <active>`
as a control request instead of failing/rebinding the active socket. Safe by default: it
requires a live compositor, does not start/stop/restart it, waits for the new main app
to attach, captures the compositor output, and writes evidence to a fresh directory.

Default launch: foot --app-id surf-ace-main-app sh -lc top
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket-path) SOCKET_PATH="${2:-}"; shift 2 ;;
    --evidence-dir) EVIDENCE_DIR="${2:-}"; shift 2 ;;
    --timeout-seconds) TIMEOUT_SECONDS="${2:-}"; shift 2 ;;
    --launch) LAUNCH_COMMAND="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

require_cmd() { command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 2; }; }
require_cmd jq
require_cmd stat
require_cmd awk
[[ -x "$CONTROL_BIN" ]] || { echo "compositor binary missing or not executable: $CONTROL_BIN" >&2; exit 2; }
[[ -n "$SOCKET_PATH" ]] || { echo "socket path must not be empty" >&2; exit 2; }
[[ -n "$LAUNCH_COMMAND" ]] || { echo "launch command must not be empty" >&2; exit 2; }
[[ -S "$SOCKET_PATH" ]] || { echo "active compositor socket not found: $SOCKET_PATH" >&2; exit 1; }

mkdir -p "$EVIDENCE_DIR"
STATUS_BEFORE="$EVIDENCE_DIR/status-before.json"
STATUS_FINAL="$EVIDENCE_DIR/status-final.json"
SERVE_STDOUT="$EVIDENCE_DIR/serve-launch.stdout"
SERVE_STDERR="$EVIDENCE_DIR/serve-launch.stderr"
CAPTURE_PATH="$EVIDENCE_DIR/capture.png"
CAPTURE_STDOUT="$EVIDENCE_DIR/capture.stdout"
CAPTURE_STDERR="$EVIDENCE_DIR/capture.stderr"
SUMMARY_PATH="$EVIDENCE_DIR/summary.json"
PROCESS_BEFORE="$EVIDENCE_DIR/socket-processes-before.txt"
PROCESS_AFTER="$EVIDENCE_DIR/socket-processes-after.txt"

socket_processes() {
  ps -eo pid=,args= | awk -v sock="$SOCKET_PATH" 'index($0, "surf-ace-compositor") && index($0, " serve ") && index($0, "--socket-path") && index($0, sock) { print }'
}
status_request() { "$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}'; }
capture_request() { "$CONTROL_BIN" capture --socket-path "$SOCKET_PATH" --output-path "$CAPTURE_PATH"; }

status_json="$(status_request 2>"$EVIDENCE_DIR/status-before.stderr")" || { echo "socket exists but did not answer get_status: $SOCKET_PATH" >&2; exit 1; }
printf '%s\n' "$status_json" > "$STATUS_BEFORE"
jq -e '.ok == true and .status.runtime.phase == "running"' "$STATUS_BEFORE" >/dev/null || { echo "preexisting compositor is not running; see $STATUS_BEFORE" >&2; exit 1; }

before_pid="$(jq -r '.status.runtime.main_app_launch_state.pid // empty' "$STATUS_BEFORE")"
socket_processes > "$PROCESS_BEFORE" || true

set +e
"$CONTROL_BIN" serve --socket-path "$SOCKET_PATH" --launch "$LAUNCH_COMMAND" >"$SERVE_STDOUT" 2>"$SERVE_STDERR"
serve_rc=$?
set -e
if [[ "$serve_rc" -ne 0 ]]; then echo "serve --launch control dispatch failed with rc=$serve_rc; see $SERVE_STDOUT and $SERVE_STDERR" >&2; exit 1; fi
jq -e '.ok == true' "$SERVE_STDOUT" >/dev/null || { echo "serve --launch did not return an ok control response; see $SERVE_STDOUT" >&2; exit 1; }

final_status=""
deadline=$((SECONDS + TIMEOUT_SECONDS))
attempt=0
while (( SECONDS < deadline )); do
  attempt=$((attempt + 1))
  status_path="$EVIDENCE_DIR/status-after-$attempt.json"
  if status_json="$(status_request 2>"$EVIDENCE_DIR/status-after-$attempt.stderr")"; then
    printf '%s\n' "$status_json" > "$status_path"
    if jq -e '.ok == true and .status.runtime.phase == "running" and .status.runtime.main_app_launch_state.state == "attached" and (.status.runtime.main_app_launch_state.pid | type == "number") and (.status.runtime.main_app_launch_intent.binding.app_id // "") == "surf-ace-main-app"' "$status_path" >/dev/null; then
      final_status="$status_path"
      break
    fi
  fi
  sleep 0.25
done
[[ -n "$final_status" ]] || { echo "timed out waiting for relaunched main app to attach; evidence: $EVIDENCE_DIR" >&2; exit 1; }
cp "$final_status" "$STATUS_FINAL"

after_pid="$(jq -r '.status.runtime.main_app_launch_state.pid // empty' "$STATUS_FINAL")"
if [[ -n "$before_pid" && "$before_pid" == "$after_pid" ]]; then echo "main app pid did not change after relaunch ($after_pid); evidence: $EVIDENCE_DIR" >&2; exit 1; fi
kill -0 "$after_pid" >/dev/null 2>&1 || { echo "attached main app pid is not alive: $after_pid" >&2; exit 1; }

capture_request >"$CAPTURE_STDOUT" 2>"$CAPTURE_STDERR" || { echo "capture failed; see $CAPTURE_STDOUT and $CAPTURE_STDERR" >&2; exit 1; }
[[ -f "$CAPTURE_PATH" ]] || { echo "capture output missing: $CAPTURE_PATH" >&2; exit 1; }
[[ "$(stat -c '%s' "$CAPTURE_PATH")" -gt 0 ]] || { echo "capture output is empty: $CAPTURE_PATH" >&2; exit 1; }

socket_processes > "$PROCESS_AFTER" || true
jq -n \
  --arg socket_path "$SOCKET_PATH" \
  --arg launch_command "$LAUNCH_COMMAND" \
  --arg evidence_dir "$EVIDENCE_DIR" \
  --arg capture_path "$CAPTURE_PATH" \
  --arg before_pid "${before_pid:-}" \
  --arg after_pid "$after_pid" \
  --slurpfile final "$STATUS_FINAL" \
  '{result:"passed", socket_path:$socket_path, launch_command:$launch_command, evidence_dir:$evidence_dir, capture_path:$capture_path, previous_main_app_pid:($before_pid | select(length > 0) | tonumber?), relaunched_main_app_pid:($after_pid | tonumber), runtime_phase:$final[0].status.runtime.phase, main_app_state:$final[0].status.runtime.main_app_launch_state.state, wayland_socket:$final[0].status.runtime.wayland_socket, note:"No compositor start/stop/restart is performed by this verifier; serve --launch must exit through the active socket control path."}' > "$SUMMARY_PATH"

echo "relaunch control proof passed"
echo "summary: $SUMMARY_PATH"
echo "capture: $CAPTURE_PATH"
