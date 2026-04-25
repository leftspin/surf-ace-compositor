# Native Pane Control Contract

Scope: this is the local compositor control contract Surf Ace can use to replace `createUnavailableNativePaneHostBridge()` with a real client. Surf Ace remains owner of pane topology, pane geometry, pane identity, content identity, and Surf Ace-rendered content. The compositor is only the native-surface hosting substrate.

## Transport

Control transport is newline-delimited JSON over the compositor Unix socket. Each request is one JSON object followed by `\n`; each response is one JSON object followed by `\n`.

Socket discovery:

- Preferred: `SURF_ACE_COMPOSITOR_SOCKET`
- Default: `/tmp/surf-ace-compositor.sock`
- CLI override: `--socket-path`

The compositor CLI now accepts `SURF_ACE_COMPOSITOR_SOCKET` for `serve`, `ctl`, `rotate`, and `capture`. Top-level `surf-ace-compositor --launch ...` also uses that env var when deciding whether to contact an already-running compositor.

## Requests

Host or relaunch pane-native content:

```json
{
  "type": "native_pane.host",
  "panes": [
    {
      "id": "pane-left",
      "content_id": "content-123",
      "binding_id": "binding-123",
      "revision": 7,
      "geometry": { "x": 0, "y": 0, "width": 640, "height": 720 },
      "target": "terminal",
      "process": { "command": "ghostty", "args": ["-e", "top"] }
    }
  ]
}
```

`native_pane.host` records the provider-owned pane plan and launches absent, failed, or exited native hosts for the supplied panes. It is idempotent for an already launching/attached pane with the same process/content/binding identity; geometry and `revision` update without relaunch.

Update pane geometry or revision without launch intent:

```json
{ "type": "native_pane.update", "panes": [ /* same pane objects */ ] }
```

Release hosted native content:

```json
{ "type": "native_pane.release", "pane_ids": ["pane-left"] }
```

Release terminates a running native process if needed, clears surface/binding evidence, and returns the pane to `surf_ace_rendered`. It does not remove the Surf Ace pane.

Bind an arriving native surface to its launched pane:

```json
{
  "type": "bind_native_pane_host_surface",
  "client_pid": 12345,
  "surface_id": 44,
  "evidence": {
    "app_id": "com.mitchellh.ghostty",
    "title": "top",
    "outcome": "not_required"
  }
}
```

The compositor reconciles by launched client PID. `app_id` and title are evidence, not authority.

## Launch Token Handshake

For compositor-spawned main apps and native pane hosts, the compositor injects an opaque `SURF_ACE_COMPOSITOR_LAUNCH_TOKEN` into the child environment. Native pane launches also receive:

- `SURF_ACE_PANE_ID`
- `SURF_ACE_NATIVE_PANE_CONTENT_ID` when present
- `SURF_ACE_NATIVE_PANE_BINDING_ID` when present
- `SURF_ACE_NATIVE_PANE_REVISION`

The raw token is not part of the control request and is not serialized in status. It is a compositor-generated binding proof that may be inherited by a daemonized or detached Wayland client.

Binding order is:

1. Exact launched PID or descendant PID.
2. Matching `SURF_ACE_COMPOSITOR_LAUNCH_TOKEN` read from `/proc/<client-pid>/environ`.
3. Otherwise deny/queue according to the normal role policy.

`app_id` and title remain status evidence only. The token path is limited to clients whose environment is readable by the compositor through procfs; Wayland core does not expose arbitrary client environment variables.

## Status Shape

Every response that returns status includes `status.panes[]`. Native-hosted panes include `nativeHost`:

```json
{
  "id": "pane-left",
  "geometry": { "x": 0, "y": 0, "width": 640, "height": 720 },
  "render_mode": { "kind": "external_native", "target": "terminal", "process": { "command": "ghostty", "args": ["-e", "top"] } },
  "external_native_state": { "state": "attached", "pid": 12345 },
  "nativeHost": {
    "paneId": "pane-left",
    "contentId": "content-123",
    "bindingId": "binding-123",
    "revision": 7,
    "surfaceId": 44,
    "lifecycle": { "state": "attached", "pid": 12345 },
    "process": { "command": "ghostty", "args": ["-e", "top"] },
    "bindingEvidence": { "app_id": "com.mitchellh.ghostty", "title": "top", "launchToken": "matched", "outcome": "not_required" }
  }
}
```

Lifecycle mapping:

- `absent`: no native host is launched for this pane.
- `launching { pid }`: launch succeeded but no surface has bound yet.
- `attached { pid }`: a surface with launched client PID is bound.
- `failed { reason }`: spawn failed.
- `exited { pid, exit_code }`: launched process exited; `exit_code` may be null.

There is no separate event stream in this slice. Surf Ace should poll `get_status` or consume status responses after mutating requests. A later slice can add subscription/event delivery without changing the request/status vocabulary above.

## Compatibility Names

Existing lower-level requests remain supported:

- `apply_native_pane_host_plan`
- `launch_native_pane_hosts`
- `bind_native_pane_host_surface`

Surf Ace should prefer `native_pane.host`, `native_pane.update`, and `native_pane.release`.
