#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="$ROOT_DIR/scripts/launch-host-seatd.sh"
CONTROL_BIN="${SURF_ACE_COMPOSITOR_BIN:-$ROOT_DIR/target/debug/surf-ace-compositor}"
SOCKET_PATH="/tmp/surf-ace-compositor.sock"
EVIDENCE_DIR="/tmp/surf-ace-visible-verify-$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECONDS=60

VERIFIER_SRC="$ROOT_DIR/scripts/surf-ace-visible-verifier.c"
VERIFIER_BUILD_DIR="$ROOT_DIR/target/visible-verifier"
VERIFIER_BIN="$VERIFIER_BUILD_DIR/surf-ace-visible-verifier"

usage() {
  cat <<'USAGE'
Usage: verify-visible-host-seatd.sh [--socket-path <path>] [--evidence-dir <dir>] [--timeout-seconds <n>]

Launches the compositor through the seatd host launcher, waits for running state,
applies 90-degree CCW output rotation, selects a fullscreen visible Wayland verifier
through the compositor control surface's exact main-app launch contract, and keeps
the session live for human verification.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket-path)
      SOCKET_PATH="${2:-}"
      shift 2
      ;;
    --evidence-dir)
      EVIDENCE_DIR="${2:-}"
      shift 2
      ;;
    --timeout-seconds)
      TIMEOUT_SECONDS="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required command not found: $1" >&2
    exit 2
  fi
}

require_cmd jq
require_cmd gcc
require_cmd wayland-scanner
require_cmd pkg-config

if [[ ! -x "$LAUNCHER" ]]; then
  echo "missing launcher: $LAUNCHER" >&2
  exit 2
fi

if [[ ! -x "$CONTROL_BIN" ]]; then
  echo "compositor binary missing: $CONTROL_BIN" >&2
  echo "build first: cargo build" >&2
  exit 2
fi

if [[ ! -f "$VERIFIER_SRC" ]]; then
  echo "missing verifier source: $VERIFIER_SRC" >&2
  exit 2
fi

other_host_runtime=()
while IFS='|' read -r pid cmdline; do
  if [[ "$cmdline" == *" serve --runtime host "* && "$cmdline" != *" --socket-path $SOCKET_PATH"* ]]; then
    other_host_runtime+=("$pid $cmdline")
  fi
done < <(ps -eo pid=,args= | awk '{pid=$1; $1=""; sub(/^ /, ""); print pid "|" $0}')

if (( ${#other_host_runtime[@]} > 0 )); then
  echo "another host runtime is already active on a different socket; stop it before visible verification:" >&2
  printf '  %s\n' "${other_host_runtime[@]}" >&2
  exit 6
fi

if ! pkg-config --exists wayland-client; then
  echo "missing wayland-client development package (pkg-config lookup failed)" >&2
  exit 2
fi

XDG_SHELL_XML="/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml"
if [[ ! -f "$XDG_SHELL_XML" ]]; then
  echo "missing protocol xml: $XDG_SHELL_XML" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_DIR" "$VERIFIER_BUILD_DIR"
COMPOSITOR_LOG="$EVIDENCE_DIR/compositor.log"
VERIFIER_LOG="$EVIDENCE_DIR/verifier.log"

COMPOSITOR_PID=""
VERIFIER_PID=""
STARTED_COMPOSITOR="false"

cleanup() {
  local rc="$?"
  if [[ -n "$VERIFIER_PID" ]] && kill -0 "$VERIFIER_PID" >/dev/null 2>&1; then
    kill "$VERIFIER_PID" >/dev/null 2>&1 || true
    wait "$VERIFIER_PID" >/dev/null 2>&1 || true
  fi
  if [[ "$STARTED_COMPOSITOR" == "true" && -n "$COMPOSITOR_PID" ]] && kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
    kill "$COMPOSITOR_PID" >/dev/null 2>&1 || true
    wait "$COMPOSITOR_PID" >/dev/null 2>&1 || true
  fi
  exit "$rc"
}

trap cleanup EXIT INT TERM

echo "evidence_dir=$EVIDENCE_DIR"

status_json=""
wayland_socket=""
if [[ -S "$SOCKET_PATH" ]]; then
  if status_json="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}' 2>/dev/null)"; then
    printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_preexisting.json"
    if [[ "$(printf '%s\n' "$status_json" | jq -r '.ok // false')" == "true" ]]; then
      phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
      preexisting_wayland_socket="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.wayland_socket // empty')"
      if [[ "$phase" == "running" && -n "$preexisting_wayland_socket" ]]; then
        printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_running.json"
        echo "reusing running compositor on socket: $SOCKET_PATH"
        wayland_socket="$preexisting_wayland_socket"
      else
        # Stale/non-running host runtime on this socket blocks a clean launch.
        while IFS='|' read -r stale_pid stale_cmdline; do
          if [[ "$stale_cmdline" == *" serve --runtime host "* && "$stale_cmdline" == *" --socket-path $SOCKET_PATH"* ]]; then
            kill "$stale_pid" >/dev/null 2>&1 || true
          fi
        done < <(ps -eo pid=,args= | awk '{pid=$1; $1=""; sub(/^ /, ""); print pid "|" $0}')
        sleep 0.5
      fi
    fi
  fi
fi

if [[ -z "$wayland_socket" ]]; then
  echo "starting compositor via seatd launcher..."
  "$LAUNCHER" --socket-path "$SOCKET_PATH" >"$COMPOSITOR_LOG" 2>&1 &
  COMPOSITOR_PID="$!"
  STARTED_COMPOSITOR="true"
  echo "compositor_pid=$COMPOSITOR_PID"

  deadline=$((SECONDS + TIMEOUT_SECONDS))
  while (( SECONDS < deadline )); do
    if ! kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
      echo "compositor exited before running; see $COMPOSITOR_LOG" >&2
      tail -n 80 "$COMPOSITOR_LOG" >&2 || true
      exit 1
    fi

    if [[ -S "$SOCKET_PATH" ]]; then
      if status_json="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}' 2>/dev/null)"; then
        printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_latest.json"
        if [[ "$(printf '%s\n' "$status_json" | jq -r '.ok // false')" == "true" ]]; then
          phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
          wayland_socket="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.wayland_socket // empty')"
          if [[ "$phase" == "running" && -n "$wayland_socket" ]]; then
            printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_running.json"
            break
          fi
        fi
      fi
    fi
    sleep 0.25
  done
fi

if [[ -z "$wayland_socket" ]]; then
  echo "timed out waiting for running status and wayland socket" >&2
  if [[ -f "$COMPOSITOR_LOG" ]]; then
    tail -n 80 "$COMPOSITOR_LOG" >&2 || true
  fi
  [[ -f "$EVIDENCE_DIR/status_latest.json" ]] && cat "$EVIDENCE_DIR/status_latest.json" >&2 || true
  exit 1
fi

rotation_response="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"set_output_rotation","rotation":"deg90"}')"
printf '%s\n' "$rotation_response" >"$EVIDENCE_DIR/rotation_response.json"
if [[ "$(printf '%s\n' "$rotation_response" | jq -r '.ok // false')" != "true" ]]; then
  echo "rotation request failed: $rotation_response" >&2
  exit 1
fi
if [[ "$(printf '%s\n' "$rotation_response" | jq -r '.status.output_rotation // empty')" != "deg90" ]]; then
  echo "rotation did not reach deg90" >&2
  cat "$EVIDENCE_DIR/rotation_response.json" >&2
  exit 1
fi

wayland-scanner client-header "$XDG_SHELL_XML" "$VERIFIER_BUILD_DIR/xdg-shell-client-protocol.h"
wayland-scanner private-code "$XDG_SHELL_XML" "$VERIFIER_BUILD_DIR/xdg-shell-protocol.c"
gcc -std=c11 -O2 -Wall -Wextra \
  -I"$VERIFIER_BUILD_DIR" \
  "$VERIFIER_SRC" \
  "$VERIFIER_BUILD_DIR/xdg-shell-protocol.c" \
  -o "$VERIFIER_BIN" \
  $(pkg-config --cflags --libs wayland-client)

MAIN_APP_LAUNCH_INTENT_JSON="$(
  jq -nc \
    --arg command "$VERIFIER_BIN" \
    '{
      process: {
        command: $command,
        args: [],
        env: {}
      },
      binding: {
        kind: "app_id",
        app_id: "surf-ace-visible-verifier"
      }
    }'
)"
printf '%s\n' "$MAIN_APP_LAUNCH_INTENT_JSON" >"$EVIDENCE_DIR/main_app_launch_intent.json"

main_app_response="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json "{\"type\":\"set_main_app_launch_intent\",\"intent\":$MAIN_APP_LAUNCH_INTENT_JSON}")"
printf '%s\n' "$main_app_response" >"$EVIDENCE_DIR/main_app_launch_response.json"
if [[ "$(printf '%s\n' "$main_app_response" | jq -r '.ok // false')" != "true" ]]; then
  echo "main-app launch-intent request failed: $main_app_response" >&2
  exit 1
fi

verifier_bound="false"
deadline=$((SECONDS + 15))
while (( SECONDS < deadline )); do
  status_json="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}')"
  printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_after_verifier_latest.json"

  phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
  rotation="$(printf '%s\n' "$status_json" | jq -r '.status.output_rotation // empty')"
  main_app_surface_id="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_surface_id // empty')"
  main_app_state="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_launch_state.state // empty')"
  VERIFIER_PID="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_launch_state.pid // empty')"

  if [[ "$phase" == "running" && "$rotation" == "deg90" && "$main_app_state" == "attached" && -n "$main_app_surface_id" && "$main_app_surface_id" != "null" ]]; then
    printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_visible_ready.json"
    verifier_bound="true"
    break
  fi
  sleep 0.25
done

if [[ "$verifier_bound" != "true" ]]; then
  echo "verifier did not bind as main app in time; see evidence under $EVIDENCE_DIR" >&2
  cat "$EVIDENCE_DIR/status_after_verifier_latest.json" >&2 || true
  exit 1
fi

cat >"$EVIDENCE_DIR/summary.txt" <<SUMMARY
phase=running
output_rotation=deg90
wayland_socket=$wayland_socket
compositor_pid=$COMPOSITOR_PID
main_app_pid=$VERIFIER_PID
evidence_dir=$EVIDENCE_DIR
SUMMARY

echo "visible verification live"
echo "summary: $EVIDENCE_DIR/summary.txt"
echo "press Ctrl-C to stop compositor + verifier"

while true; do
  if [[ "$STARTED_COMPOSITOR" == "true" && -n "$COMPOSITOR_PID" ]] && ! kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
    echo "compositor exited; see $COMPOSITOR_LOG" >&2
    exit 1
  fi
  if [[ -n "$VERIFIER_PID" ]] && ! kill -0 "$VERIFIER_PID" >/dev/null 2>&1; then
    echo "verifier exited; see latest status under $EVIDENCE_DIR" >&2
    exit 1
  fi
  sleep 1
done
