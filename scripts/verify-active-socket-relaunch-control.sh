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
require_cmd python3
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

analyze_capture() {
  python3 - "$1" <<'PY'
import json, struct, sys, zlib
path = sys.argv[1]
raw = open(path, "rb").read()
if not raw.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit("capture is not a PNG")
pos = 8
width = height = bit_depth = color_type = None
idat = bytearray()
while pos + 8 <= len(raw):
    length = struct.unpack(">I", raw[pos:pos + 4])[0]
    chunk_type = raw[pos + 4:pos + 8]
    data = raw[pos + 8:pos + 8 + length]
    pos += 12 + length
    if chunk_type == b"IHDR":
        width, height, bit_depth, color_type, compression, filter_method, interlace = struct.unpack(">IIBBBBB", data)
        if bit_depth != 8 or compression != 0 or filter_method != 0 or interlace != 0:
            raise SystemExit("unsupported PNG format")
    elif chunk_type == b"IDAT":
        idat.extend(data)
    elif chunk_type == b"IEND":
        break
if not width or not height or not idat:
    raise SystemExit("invalid PNG")
channels_by_type = {0: 1, 2: 3, 4: 2, 6: 4}
channels = channels_by_type.get(color_type)
if channels is None:
    raise SystemExit("unsupported PNG color type")
stride = width * channels
compressed = zlib.decompress(bytes(idat))
rows = []
prev = bytearray(stride)
off = 0
for _ in range(height):
    filt = compressed[off]
    off += 1
    row = bytearray(compressed[off:off + stride])
    off += stride
    for i in range(stride):
        left = row[i - channels] if i >= channels else 0
        up = prev[i]
        up_left = prev[i - channels] if i >= channels else 0
        if filt == 1:
            row[i] = (row[i] + left) & 255
        elif filt == 2:
            row[i] = (row[i] + up) & 255
        elif filt == 3:
            row[i] = (row[i] + ((left + up) >> 1)) & 255
        elif filt == 4:
            p = left + up - up_left
            pa, pb, pc = abs(p - left), abs(p - up), abs(p - up_left)
            row[i] = (row[i] + (left if pa <= pb and pa <= pc else up if pb <= pc else up_left)) & 255
        elif filt != 0:
            raise SystemExit("unsupported PNG filter")
    rows.append(row)
    prev = row
values = []
edge_sum = 0.0
edge_count = 0
bright = 0
bins = set()
prev_gray_row = None
for row in rows:
    gray_row = []
    for x in range(width):
        px = row[x * channels:(x + 1) * channels]
        if color_type in (0, 4):
            g = px[0]
        else:
            g = (299 * px[0] + 587 * px[1] + 114 * px[2]) // 1000
        values.append(g)
        gray_row.append(g)
        bins.add(g // 8)
        if g >= 96:
            bright += 1
        if x:
            edge_sum += abs(g - gray_row[x - 1])
            edge_count += 1
        if prev_gray_row is not None:
            edge_sum += abs(g - prev_gray_row[x])
            edge_count += 1
    prev_gray_row = gray_row
count = len(values)
avg = sum(values) / count
variance = sum((v - avg) ** 2 for v in values) / count
std = variance ** 0.5
edge = edge_sum / edge_count if edge_count else 0.0
bright_fraction = bright / count
unique_bins = len(bins)
passes = std >= 8.0 and edge >= 1.0 and unique_bins >= 8 and bright_fraction >= 0.001
print(json.dumps({"avg": round(avg, 2), "std": round(std, 2), "edge": round(edge, 2), "unique_bins": unique_bins, "bright_fraction": round(bright_fraction, 6), "passes_visual_gate": passes}))
raise SystemExit(0 if passes else 1)
PY
}

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

capture_ok=""
for capture_attempt in $(seq 1 "$TIMEOUT_SECONDS"); do
  attempt_capture_path="$EVIDENCE_DIR/capture-$capture_attempt.png"
  CAPTURE_PATH="$attempt_capture_path" capture_request >"$CAPTURE_STDOUT" 2>"$CAPTURE_STDERR" || { echo "capture attempt $capture_attempt failed; see $CAPTURE_STDOUT and $CAPTURE_STDERR" >&2; exit 1; }
  [[ -f "$attempt_capture_path" ]] || { echo "capture output missing: $attempt_capture_path" >&2; exit 1; }
  [[ "$(stat -c '%s' "$attempt_capture_path")" -gt 0 ]] || { echo "capture output is empty: $attempt_capture_path" >&2; exit 1; }
  stats_path="$EVIDENCE_DIR/capture-$capture_attempt.stats.json"
  if analyze_capture "$attempt_capture_path" >"$stats_path"; then
    cp "$attempt_capture_path" "$CAPTURE_PATH"
    cp "$stats_path" "$EVIDENCE_DIR/capture.stats.json"
    capture_ok="$attempt_capture_path"
    break
  fi
  sleep 1
done
[[ -n "$capture_ok" ]] || { echo "timed out waiting for visually nonblank capture; evidence: $EVIDENCE_DIR" >&2; exit 1; }

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
