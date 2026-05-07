#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="$ROOT_DIR/scripts/launch-host-seatd.sh"
CONTROL_BIN="${SURF_ACE_COMPOSITOR_BIN:-$ROOT_DIR/target/debug/surf-ace-compositor}"
SOCKET_PATH="/tmp/surf-ace-compositor.sock"
EVIDENCE_DIR="/tmp/surf-ace-zsh-main-verify-$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECONDS=60
APP_ID="surf-ace-zsh-verifier"
TERMINAL=""
MAIN_APP_START_DELAY_SECONDS="${SURF_ACE_MAIN_APP_START_DELAY_SECONDS:-1}"

usage() {
  cat <<'USAGE'
Usage: verify-zsh-main-host-seatd.sh [--socket-path <path>] [--evidence-dir <dir>] [--timeout-seconds <n>] [--terminal <name>]

Launches the compositor through the seatd host launcher, waits for running state,
applies 90-degree CCW output rotation, selects a supported Wayland terminal as the
fullscreen main app through the compositor's explicit launch contract, runs zsh
inside it, and keeps the session live for operator
verification.

Supported terminal names: foot, ghostty, kitty, wezterm, alacritty
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
    --terminal)
      TERMINAL="${2:-}"
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

select_terminal() {
  case "$TERMINAL" in
    "")
      if command -v foot >/dev/null 2>&1; then
        TERMINAL="foot"
      elif command -v ghostty >/dev/null 2>&1; then
        TERMINAL="ghostty"
      elif command -v kitty >/dev/null 2>&1; then
        TERMINAL="kitty"
      elif command -v wezterm >/dev/null 2>&1; then
        TERMINAL="wezterm"
      elif command -v alacritty >/dev/null 2>&1; then
        TERMINAL="alacritty"
      else
        echo "no supported Wayland terminal found; install one of: foot ghostty kitty wezterm alacritty" >&2
        exit 2
      fi
      ;;
    foot|ghostty|kitty|wezterm|alacritty)
      if ! command -v "$TERMINAL" >/dev/null 2>&1; then
        echo "requested terminal is not installed: $TERMINAL" >&2
        exit 2
      fi
      ;;
    *)
      echo "unsupported terminal: $TERMINAL" >&2
      usage >&2
      exit 2
      ;;
  esac
}

terminal_binding_app_id() {
  case "$TERMINAL" in
    ghostty)
      # Observed on the live Wayland socket: Ghostty keeps app_id fixed to
      # com.mitchellh.ghostty even when launched with --class=<value>.
      printf '%s\n' 'com.mitchellh.ghostty'
      ;;
    *)
      printf '%s\n' "$APP_ID"
      ;;
  esac
}

terminal_command() {
  case "$TERMINAL" in
    foot)
      printf '%s\0' foot --app-id "$APP_ID" /bin/sh -lc "sleep $MAIN_APP_START_DELAY_SECONDS; exec zsh -i"
      ;;
    ghostty)
      printf '%s\0' ghostty --class="$APP_ID" -e /bin/sh -lc "sleep $MAIN_APP_START_DELAY_SECONDS; exec zsh -i"
      ;;
    kitty)
      printf '%s\0' kitty --class "$APP_ID" /bin/sh -lc "sleep $MAIN_APP_START_DELAY_SECONDS; exec zsh -i"
      ;;
    wezterm)
      printf '%s\0' wezterm start --class "$APP_ID" -- /bin/sh -lc "sleep $MAIN_APP_START_DELAY_SECONDS; exec zsh -i"
      ;;
    alacritty)
      printf '%s\0' alacritty --class "$APP_ID","$APP_ID" -e /bin/sh -lc "sleep $MAIN_APP_START_DELAY_SECONDS; exec zsh -i"
      ;;
  esac
}



require_cmd jq
require_cmd zsh

if [[ ! -x "$LAUNCHER" ]]; then
  echo "missing launcher: $LAUNCHER" >&2
  exit 2
fi

if [[ ! -x "$CONTROL_BIN" ]]; then
  echo "compositor binary missing: $CONTROL_BIN" >&2
  echo "build first: cargo build" >&2
  exit 2
fi

select_terminal

BINDING_APP_ID="$(terminal_binding_app_id)"
mapfile -d '' -t TERMINAL_COMMAND < <(terminal_command)
MAIN_APP_LAUNCH_INTENT_JSON="$(
  jq -nc \
    --arg command "${TERMINAL_COMMAND[0]}" \
    --arg app_id "$BINDING_APP_ID" \
    --args "${TERMINAL_COMMAND[@]:1}" '
      {
        process: {
          command: $command,
          args: $ARGS.positional,
          env: {}
        },
        binding: {
          kind: "app_id",
          app_id: $app_id
        }
      }
    '
)"

other_host_runtime=()
while IFS='|' read -r pid cmdline; do
  if [[ "$cmdline" == *" serve --runtime host "* && "$cmdline" != *" --socket-path $SOCKET_PATH"* ]]; then
    other_host_runtime+=("$pid $cmdline")
  fi
done < <(ps -eo pid=,args= | awk '{pid=$1; $1=""; sub(/^ /, ""); print pid "|" $0}')

if (( ${#other_host_runtime[@]} > 0 )); then
  echo "another host runtime is already active on a different socket; stop it before zsh verification:" >&2
  printf '  %s\n' "${other_host_runtime[@]}" >&2
  exit 6
fi

mkdir -p "$EVIDENCE_DIR"
printf '%s\n' "$MAIN_APP_LAUNCH_INTENT_JSON" >"$EVIDENCE_DIR/main_app_launch_intent.json"
COMPOSITOR_LOG="$EVIDENCE_DIR/compositor.log"

COMPOSITOR_PID=""
MAIN_APP_PID=""
STARTED_COMPOSITOR="false"

cleanup() {
  local rc="$?"
  if [[ -n "$MAIN_APP_PID" ]] && kill -0 "$MAIN_APP_PID" >/dev/null 2>&1; then
    kill "$MAIN_APP_PID" >/dev/null 2>&1 || true
    wait "$MAIN_APP_PID" >/dev/null 2>&1 || true
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
  if status_json="$($CONTROL_BIN ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}' 2>/dev/null)"; then
    printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_preexisting.json"
    if [[ "$(printf '%s\n' "$status_json" | jq -r '.ok // false')" == "true" ]]; then
      phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
      preexisting_wayland_socket="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.wayland_socket // empty')"
      if [[ "$phase" == "running" && -n "$preexisting_wayland_socket" ]]; then
        printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_running.json"
        echo "reusing running compositor on socket: $SOCKET_PATH"
        wayland_socket="$preexisting_wayland_socket"
      else
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
  "$LAUNCHER" \
    --socket-path "$SOCKET_PATH" \
    --main-app-launch-intent-json "$MAIN_APP_LAUNCH_INTENT_JSON" \
    >"$COMPOSITOR_LOG" 2>&1 &
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
      if status_json="$($CONTROL_BIN ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}' 2>/dev/null)"; then
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
  [[ -f "$COMPOSITOR_LOG" ]] && tail -n 80 "$COMPOSITOR_LOG" >&2 || true
  [[ -f "$EVIDENCE_DIR/status_latest.json" ]] && cat "$EVIDENCE_DIR/status_latest.json" >&2 || true
  exit 1
fi

if [[ "$STARTED_COMPOSITOR" != "true" ]]; then
  main_app_response="$($CONTROL_BIN ctl --socket-path "$SOCKET_PATH" --request-json "{\"type\":\"set_main_app_launch_intent\",\"intent\":$MAIN_APP_LAUNCH_INTENT_JSON}")"
  printf '%s\n' "$main_app_response" >"$EVIDENCE_DIR/main_app_launch_response.json"
  if [[ "$(printf '%s\n' "$main_app_response" | jq -r '.ok // false')" != "true" ]]; then
    echo "main-app launch-intent request failed: $main_app_response" >&2
    exit 1
  fi
fi

rotation_response="$($CONTROL_BIN ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"set_output_rotation","rotation":"deg90"}')"
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

main_bound="false"
deadline=$((SECONDS + 15))
while (( SECONDS < deadline )); do
  status_json="$($CONTROL_BIN ctl --socket-path "$SOCKET_PATH" --request-json '{"type":"get_status"}')"
  printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_after_terminal_latest.json"

  phase="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.phase // empty')"
  rotation="$(printf '%s\n' "$status_json" | jq -r '.status.output_rotation // empty')"
  main_app_surface_id="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_surface_id // empty')"
  main_app_state="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_launch_state.state // empty')"
  MAIN_APP_PID="$(printf '%s\n' "$status_json" | jq -r '.status.runtime.main_app_launch_state.pid // empty')"

  if [[ "$phase" == "running" && "$rotation" == "deg90" && "$main_app_state" == "attached" && -n "$main_app_surface_id" && "$main_app_surface_id" != "null" ]]; then
    printf '%s\n' "$status_json" >"$EVIDENCE_DIR/status_zsh_ready.json"
    main_bound="true"
    break
  fi
  sleep 0.25
done

if [[ "$main_bound" != "true" ]]; then
  echo "terminal did not bind as main app in time; see evidence under $EVIDENCE_DIR" >&2
  cat "$EVIDENCE_DIR/status_after_terminal_latest.json" >&2 || true
  exit 1
fi

cat >"$EVIDENCE_DIR/summary.txt" <<SUMMARY
phase=running
output_rotation=deg90
main_app_terminal=$TERMINAL
main_app_binding=app_id:$BINDING_APP_ID
main_app_pid=$MAIN_APP_PID
wayland_socket=$wayland_socket
SUMMARY

echo "zsh main-app verification ready; status evidence in $EVIDENCE_DIR"
echo "Press Ctrl-C when finished inspecting the rotated zsh session."
while true; do
  if [[ "$STARTED_COMPOSITOR" == "true" && -n "$COMPOSITOR_PID" ]] && ! kill -0 "$COMPOSITOR_PID" >/dev/null 2>&1; then
    echo "compositor exited; see $COMPOSITOR_LOG" >&2
    exit 1
  fi
  if [[ -n "$MAIN_APP_PID" ]] && ! kill -0 "$MAIN_APP_PID" >/dev/null 2>&1; then
    echo "main app exited; see latest status under $EVIDENCE_DIR" >&2
    exit 1
  fi
  sleep 1
done
