#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="$ROOT_DIR/scripts/launch-host-seatd.sh"
CONTROL_BIN="${SURF_ACE_COMPOSITOR_BIN:-$ROOT_DIR/target/debug/surf-ace-compositor}"
SOCKET_PATH="/tmp/surf-ace-compositor.sock"
EVIDENCE_DIR="/tmp/surf-ace-visible-verify-$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECONDS=60

DEMO_SRC="$ROOT_DIR/scripts/surf-ace-visible-demo.c"
DEMO_BUILD_DIR="$ROOT_DIR/target/visible-demo"
DEMO_BIN="$DEMO_BUILD_DIR/surf-ace-visible-demo"

usage() {
  cat <<'USAGE'
Usage: verify-visible-host-seatd.sh [--socket-path <path>] [--evidence-dir <dir>] [--timeout-seconds <n>]

Launches the compositor through the seatd host launcher, waits for running state,
applies 90-degree CCW output rotation, starts a fullscreen visible Wayland demo,
and keeps the session live for human verification.
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

if [[ ! -f "$DEMO_SRC" ]]; then
  echo "missing demo source: $DEMO_SRC" >&2
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

mkdir -p "$EVIDENCE_DIR" "$DEMO_BUILD_DIR"
COMPOSITOR_LOG="$EVIDENCE_DIR/compositor.log"
DEMO_LOG="$EVIDENCE_DIR/demo.log"

COMPOSITOR_PID=""
DEMO_PID=""
STARTED_COMPOSITOR="false"

cleanup() {
  local rc="$?"
  if [[ -n "$DEMO_PID" ]] && kill -0 "$DEMO_PID" >/dev/null 2>&1; then
    kill "$DEMO_PID" >/dev/null 2>&1 || true
    wait "$DEMO_PID" >/dev/null 2>&1 || true
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

wayland-scanner client-header "$XDG_SHELL_XML" "$DEMO_BUILD_DIR/xdg-shell-client-protocol.h"
wayland-scanner private-code "$XDG_SHELL_XML" "$DEMO_BUILD_DIR/xdg-shell-protocol.c"
gcc -std=c11 -O2 -Wall -Wextra \
  -I"$DEMO_BUILD_DIR" \
  "$DEMO_SRC" \
  "$DEMO_BUILD_DIR/xdg-shell-protocol.c" \
  -o "$DEMO_BIN" \
  $(pkg-config --cflags --libs wayland-client)

UID_VALUE="$(id -u)"
XDG_RUNTIME_DIR_VALUE="${XDG_RUNTIME_DIR:-/run/user/$UID_VALUE}"
if [[ ! -d "$XDG_RUNTIME_DIR_VALUE" ]]; then
  echo "missing XDG runtime dir: $XDG_RUNTIME_DIR_VALUE" >&2
  exit 1
fi

echo "starting visible demo on WAYLAND_DISPLAY=$wayland_socket"
env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR_VALUE" WAYLAND_DISPLAY="$wayland_socket" \
  "$DEMO_BIN" >"$DEMO_LOG" 2>&1 &
DEMO_PID="$!"
echo "demo_pid=$DEMO_PID"

demo_bound="false"
deadline=$((SECONDS + 15))
while (( SECONDS < deadline )); do
  if ! kill -0 "$DEMO_PID" >/dev/null 2>&1; then
    echo "visible demo exited unexpectedly; see $DEMO_LOG" >&2
    tail -n 40 "$DEMO_LOG" >&2 || true
    exit 1
  fi

  status_json="$("$CONTROL_BIN" ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}')"
  printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_after_demo_latest.json"

  phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
  rotation="$(printf '%s\n' "$status_json" | jq -r '.status.output_rotation // empty')"
  main_app_surface_id="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_surface_id // empty')"

  if [[ "$phase" == "running" && "$rotation" == "deg90" && -n "$main_app_surface_id" && "$main_app_surface_id" != "null" ]]; then
    printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_visible_ready.json"
    demo_bound="true"
    break
  fi
  sleep 0.25
done

if [[ "$demo_bound" != "true" ]]; then
  echo "demo did not bind as main app in time; see evidence under $EVIDENCE_DIR" >&2
  cat "$EVIDENCE_DIR/status_after_demo_latest.json" >&2 || true
  exit 1
fi

cat >"$EVIDENCE_DIR/summary.txt" <<SUMMARY
phase=running
output_rotation=deg90
wayland_socket=$wayland_socket
compositor_pid=$COMPOSITOR_PID
demo_pid=$DEMO_PID
evidence_dir=$EVIDENCE_DIR
SUMMARY

echo "visible verification live"
echo "summary: $EVIDENCE_DIR/summary.txt"
echo "press Ctrl-C to stop compositor + demo"

while true; do
  if ! kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
    echo "compositor exited; see $COMPOSITOR_LOG" >&2
    exit 1
  fi
  if ! kill -0 "$DEMO_PID" >/dev/null 2>&1; then
    echo "demo exited; see $DEMO_LOG" >&2
    exit 1
  fi
  sleep 1
done
