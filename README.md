# Surf Ace Compositor

First implementation passes for the Surf Ace Linux compositor seam.

The current implementation ships:

- provider topology authority (pane geometry is only accepted via provider snapshot updates)
- explicit pane mode switching between Surf Ace-rendered and `external/native` content
- terminal-first external target with exec/process payload (never modeled as `html`)
- reversible runtime switching with external process lifecycle states
- a tiny Unix-socket control path for both bootstrap operations (rotation/status) and runtime pane mode operations
- explicit prototype policy: one active overlay pane at a time, separated from the long-term per-pane hosting contract
- Smithay-based local runtime bring-up path using `winit` (real compositor event loop + redraw loop + Wayland socket listener skeleton)
- Smithay-based host runtime preflight path using `libseat` + `udev` + DRM device opening:
  - dedicated `host` runtime mode separate from `winit`
  - creates a real `libseat` session
  - monitors seat-scoped DRM devices via `udev`
  - attempts compositor-owned opening of DRM card devices through `libseat` session
  - reports host seat/device snapshot in runtime status (`host_seat_name`, detected/opened DRM counts, primary opened device path)
- real xdg-shell handling in runtime:
  - toplevel role assignment (`main_app` fullscreen slot, `overlay_native` slot, deny extra independent toplevels)
  - deterministic main-app binding via configurable `main_app_match_hint` (default: `surf-ace`) with pending-identity queueing
  - runtime/product bridge guard: `overlay_native` role only binds when state has an active external/native overlay pane in expected lifecycle state
  - runtime/product bridge lifecycle: overlay role attach/detach transitions active overlay pane state between `launching` and `attached`
  - runtime status now reports `overlay_bound_pane_id` when an overlay role surface is bound
  - popup acceptance only when attached to a known role owner, with positioner-based constraint into owner bounds
  - fuller pointer + keyboard focus routing semantics across main/overlay/popup ownership
  - backend-applied output rotation transform in the render path

## Run

Start control/core daemon only:

```bash
cargo run -- serve --runtime none --socket-path /tmp/surf-ace-compositor.sock
```

Start with Smithay `winit` runtime bring-up:

```bash
cargo run -- serve --runtime winit --socket-path /tmp/surf-ace-compositor.sock
```

Note: `--runtime winit` requires a graphical host session (Wayland or X11). In headless environments, winit initialization fails before serving.

Start host-compositor backend preflight (`libseat` + `udev` + DRM):

```bash
cargo run -- serve --runtime host --socket-path /tmp/surf-ace-compositor.sock
```

Note: `--runtime host` requires host-compositor permissions for seat-managed DRM device access (for example through seatd/systemd-logind policy). In restricted environments, startup may fail after detecting DRM nodes but before opening them.

Operator launcher (seatd, handles stale-shell group inheritance):

```bash
./scripts/launch-host-seatd.sh --socket-path /tmp/surf-ace-compositor.sock
```

Operator visible verification (launch + 90-degree CCW rotation + fullscreen demo app):

```bash
./scripts/verify-visible-host-seatd.sh --socket-path /tmp/surf-ace-compositor.sock
```

This command keeps the compositor and demo running until `Ctrl-C`, so the physical
screen can be checked directly. Evidence files are written to `/tmp/surf-ace-visible-verify-*`.

## Operator Quick Path (Racter)

Run from a local tty login on the real machine (for example `tty1` or `tty2`).
Use one socket consistently:

```bash
export SURF_ACE_SOCKET=/tmp/surf-ace-compositor.sock
```

Do not wrap the verify command in `timeout` or another session-killer when a human is
checking the screen.

Launch only (no demo, no forced rotation):

```bash
./scripts/launch-host-seatd.sh --socket-path "$SURF_ACE_SOCKET"
```

Visible verification (recommended):

```bash
./scripts/verify-visible-host-seatd.sh --socket-path "$SURF_ACE_SOCKET"
```

Stale same-socket recovery behavior:

```text
If an earlier run on the same socket is paused/failed (for example host session paused),
verify-visible-host-seatd.sh tears down that stale runtime and relaunches cleanly on the
same socket before applying rotation and starting the demo.
```

Expected on screen (success criteria):

```text
The display leaves the text console, rotates 90 degrees CCW, and shows a fullscreen
animated "Surf Ace Visible Demo". The image stays visible until Ctrl-C is pressed in
the verify command session.
```

Send control requests:

```bash
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"get_status"}'
```

Set/clear runtime focus target skeleton:

```bash
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"set_runtime_focus_target","target":"overlay_native"}'
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"clear_runtime_focus_target"}'
```

Set deterministic Surf Ace main-app match hint:

```bash
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"set_runtime_main_app_match_hint","hint":"surf"}'
```

Example provider snapshot + external/native switch:

```bash
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"apply_provider_snapshot","panes":[{"id":"pane-1","geometry":{"x":0,"y":0,"width":1280,"height":800}}]}'
cargo run -- ctl --socket-path /tmp/surf-ace-compositor.sock --request-json '{"type":"switch_pane_to_external_native","pane_id":"pane-1","target":"terminal","process":{"command":"/bin/sh","args":["-lc","sleep 5"]}}'
```

## License

Apache-2.0. See `LICENSE`.

## Validate

```bash
cargo fmt
cargo test
```
