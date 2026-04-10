#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DEFAULT="$ROOT_DIR/target/debug/surf-ace-compositor"

SOCKET_PATH="/tmp/surf-ace-compositor.sock"
BIN_PATH="${SURF_ACE_COMPOSITOR_BIN:-$BIN_DEFAULT}"

usage() {
  cat <<'USAGE'
Usage: launch-host-seatd.sh [--socket-path <path>] [--bin <path>]

Starts Surf Ace host runtime with seatd mediation, handling stale-shell
group inheritance by entering the seatd socket group for this command.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket-path)
      SOCKET_PATH="${2:-}"
      shift 2
      ;;
    --bin)
      BIN_PATH="${2:-}"
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

if [[ ! -x "$BIN_PATH" ]]; then
  echo "compositor binary is not executable: $BIN_PATH" >&2
  echo "build first: cargo build" >&2
  exit 2
fi

if ! systemctl is-active --quiet seatd; then
  echo "seatd is not active; start it first: sudo systemctl enable --now seatd" >&2
  exit 3
fi

if [[ ! -S /run/seatd.sock ]]; then
  echo "seatd socket not found at /run/seatd.sock" >&2
  exit 3
fi

SOCKET_GROUP="$(stat -c '%G' /run/seatd.sock)"
UID_VALUE="$(id -u)"
XDG_RUNTIME_DIR_VALUE="${XDG_RUNTIME_DIR:-/run/user/$UID_VALUE}"

if [[ ! -d "$XDG_RUNTIME_DIR_VALUE" ]]; then
  echo "XDG runtime dir does not exist: $XDG_RUNTIME_DIR_VALUE" >&2
  exit 4
fi

TTY_PATH="$(tty 2>/dev/null || true)"
if [[ -z "$TTY_PATH" || "$TTY_PATH" == "not a tty" || "$TTY_PATH" == /dev/pts/* ]]; then
  echo "warning: launch is not on a local VT (${TTY_PATH:-none}); for real-screen verification, run from a local tty login (for example tty1)." >&2
fi

run_direct() {
  exec env \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR_VALUE" \
    LIBSEAT_BACKEND=seatd \
    "$BIN_PATH" serve --runtime host --socket-path "$SOCKET_PATH"
}

if id -nG | tr ' ' '\n' | grep -qx "$SOCKET_GROUP"; then
  run_direct
fi

if id -nG "$USER" | tr ' ' '\n' | grep -qx "$SOCKET_GROUP"; then
  echo "current shell missing '$SOCKET_GROUP'; entering group for this launch" >&2
  CMD="$(printf 'XDG_RUNTIME_DIR=%q LIBSEAT_BACKEND=seatd %q serve --runtime host --socket-path %q' \
    "$XDG_RUNTIME_DIR_VALUE" "$BIN_PATH" "$SOCKET_PATH")"
  exec sg "$SOCKET_GROUP" -c "$CMD"
fi

echo "user '$USER' is not in required group '$SOCKET_GROUP' for /run/seatd.sock" >&2
echo "add membership: sudo usermod -aG $SOCKET_GROUP $USER" >&2
echo "then start a new login session (or use 'sg $SOCKET_GROUP -c ...')." >&2
exit 5
