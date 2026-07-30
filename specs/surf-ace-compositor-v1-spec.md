# Surf Ace Compositor V1 Spec

## T1770 topology-authority amendment

For lockless-capable Surf Ace clients, the Surf Ace client owns topology truth and
the ordered mutation seam for every surface it renders. Controllers submit
identity-independent intent to that client. The compositor receives only
client-resolved pane identity, geometry, content-mode, and native-hosting state
after the client commits; it realizes that state and never sequences, validates,
reconstructs, or persists controller topology intent.

This amendment supersedes every use of “provider topology authority” or
“provider-owned geometry” in earlier revisions of this document. It does not
promote the compositor: display/output policy, input routing, and native-surface
hosting remain compositor responsibilities, while pane semantics and topology
remain client truth.

## Goal

Define the first implementation-ready Linux compositor mode for Surf Ace without changing Surf Ace’s normal app-mode product shape on existing desktops.

The chosen direction is fixed for this spec:

- Rust + Smithay
- normal Surf Ace app mode remains unchanged
- Linux gets an optional host/compositor mode
- Electron is not the compositor
- v1 host mode must be able to run Surf Ace itself as the main app inside the compositor
- v1 host mode is one fullscreen main app plus one overlay app layer for the first product slice, but the long-term pane-hosting model is per-pane and dynamic
- output rotation is supported in host mode
- the first honest external-surface target is a terminal/CLI app
- hosted native apps become an explicit external/native surface content type instead of being modeled as `html`
- Surf Ace must be able to detect when it is running under the compositor host mode
- Surf Ace must be able to tell the compositor which app to run in a pane, and switch that pane dynamically between Surf Ace-rendered content and external/native surface hosting
- default runtime behavior should be mostly automatic, while still exposing narrow operator/developer overrides when automatic selection is wrong

## Product Shape

Surf Ace continues to have two product modes on Linux:

1. **Normal app mode**
   - Surf Ace runs as a normal app inside any installed compositor or desktop session.
   - This mode is unchanged by the compositor project.

2. **Optional host/compositor mode**
   - Surf Ace runs with a dedicated Linux compositor process that owns the output stack.
   - This mode is Linux-only and optional.
   - In host mode, the compositor should automatically choose the correct backend, render device, and active output whenever it can do so with high confidence.
   - When automatic choice is wrong or the environment is unusual, operators and developers must be able to override backend, device, and output selection explicitly.

## V1 Overlay Policy vs Long-Term Contract

These two truths must not be collapsed into each other:

- **First product-slice policy**
  - one fullscreen Surf Ace main app
  - one overlay app layer
  - terminal/CLI as the first honest native-hosted target

- **Long-term product contract**
  - any pane may dynamically switch between Surf Ace-rendered content and native-hosted content
  - native-hosted content remains under Surf Ace client pane authority
  - the compositor realizes native surfaces inside client-resolved pane rectangles

The v1 overlay policy is a narrow proving slice. It is not the long-term limit of the pane-hosting model.

## Spec Invariants

These invariants are required even if implementation details change:

1. Surf Ace must be able to run as the main app inside the compositor, even though it is an Electron app.
2. Electron may be the main client, but it may not become the compositor.
3. Surf Ace must be able to detect that it is running under Surf Ace compositor host mode.
4. Pane geometry and pane identity remain Surf Ace client truth; controllers submit intent and the compositor must not become a second topology authority.
5. Any pane may dynamically switch between Surf Ace-rendered content and an external/native surface.
6. Surf Ace must be able to tell the compositor which app to run in a pane when that pane switches to an external/native surface.
7. For native-hosted panes, the payload primitive is an executable/process spec supplied under Surf Ace authority (command + args, with optional cwd/env), not a separate app/surface authority model.
8. The switch between Surf Ace-rendered content and external/native surface hosting must be live and reversible at runtime, not one-time startup configuration.
9. External/native surfaces must be represented explicitly, never as `html`.
10. Rotation remains a compositor/output concern, not a controller content concern.
11. The first product slice may use a single terminal/CLI target, but the pane-hosting abstraction must already support future standard Linux GUI apps.
12. For tmux/terminal targets, persistent session state may survive outside the pane binding even when the native surface attachment is destroyed and recreated.
13. External/native hosting is a pane content mode under Surf Ace authority, not a peer authority.
14. Discovery and pairing remain attached to the Surf Ace app/surface instance, not to hosted child apps inside panes.
15. External/native panes must have an explicit reduced or adapted event contract rather than inheriting HTML-centric event semantics by accident.
16. Focus, input, selection, and annotation ownership must be explicit whenever a pane switches between Surf Ace-rendered and external/native content.
17. A pane-hosted native app gets one compositor-managed surface slot in v1.
18. Transient child surfaces (menus, tooltips, true dialogs/popups) may remain attached to that pane if they are dependent on the hosted app surface.
19. Attached transient child surfaces may visually float above pane content, but they remain owned by that pane and must be clipped or repositioned within pane policy rather than escaping into global desktop space.
20. Independent additional top-level windows must not silently escape into free-floating window-manager behavior in v1, they must be denied, collapsed into pane policy, or treated as unsupported.
21. The compositor must expose a tiny direct bootstrap/control path for setup and recovery operations such as output rotation, so sideways-monitor bring-up does not depend on Surf Ace already being usable.
22. Surf Ace should use that same compositor control path once running, optional network control, if added later, must be a thin wrapper over the same underlying control surface rather than a second authority path.
23. That same compositor control path must support a host-local screen-capture operation that writes the current compositor output to an image file on disk, so remote debugging can inspect what the panel is showing without adding a second display or control authority.
24. In host mode, the compositor must detect when the active physical display connection moves to a different connector or port and switch or rebind output ownership to the newly active path when that is safe, if it cannot, it must fail closed with explicit status instead of silently continuing on a stale output path.
25. Automatic runtime selection is the default product behavior, but backend, device, and output overrides must exist as explicit escape hatches.
26. Every automatic fallback, degraded path, rejected override, or required manual recovery must be logged clearly with the attempted choice, the chosen fallback, and the reason.

## Operating Principle: Automatic First, Explicit When Needed

The compositor should behave more like a kiosk appliance than a toolkit exercise:

- by default it should auto-detect the viable backend
- by default it should auto-pick the render or DRM device it intends to own
- by default it should auto-pick the active output path and preferred mode
- by default it should auto-recover from ordinary connector changes when recovery is safe and does not broaden authority
- it should only require explicit operator input when ambiguity, safety, or repeated failure makes automatic recovery unreliable

That automatic-first behavior is required for the product shape, not a convenience feature.

At the same time, v1 must not trap operators inside wrong heuristics. Explicit overrides are required for:

- runtime or backend selection, for example local development backend vs host KMS path
- DRM or render device selection when multiple GPUs or render nodes exist
- output selection when the wrong panel or connector is chosen automatically
- rotation and mode recovery during bootstrap and remote debugging

Override policy:

1. Overrides are narrow escape hatches, not a second operating model.
2. Overrides may constrain or replace auto-selection, but they do not change Surf Ace client topology authority.
3. If an override is invalid, unavailable, or unsafe, the compositor must say so explicitly and fail closed or fall back according to documented policy.
4. Successful override use must be visible in status and logs so operators know the compositor is running in a forced mode.

## V1 Success Criteria

V1 is successful when all of the following are true:

1. A dedicated Surf Ace compositor can run in a local development backend and in real host/compositor mode on Linux.
2. The compositor can show exactly one fullscreen Surf Ace app and one overlay app layer at the same time.
3. The compositor can rotate the output while preserving the expected fullscreen or overlay arrangement.
4. The first overlay target is a Wayland-native terminal or CLI app.
5. The compositor defaults to automatic backend, device, and output selection, while still supporting explicit overrides for debugging and recovery.
6. Connector moves and ordinary hotplug changes are handled automatically when safe, with explicit logging when the compositor falls back or cannot rebind in-process.
7. The controller-facing model remains recognizably Surf Ace:
   - controllers still target windows and stable panes by submitting intent to the client authority
   - compositor policy does not become a controller or topology authority
   - hosted native overlay content is represented explicitly, not smuggled through `html`

## Non-Goals

This spec does not include:

- replacing normal Surf Ace app mode on Linux
- making Electron the compositor
- general desktop shell features
- multi-overlay stacking
- arbitrary window management
- Xwayland support
- native app embedding inside Electron
- controller-originated compositor control beyond client-committed main-content vs overlay-content state
- generalized app sandboxing or app-store style policy
- a cross-platform compositor abstraction for macOS or iPad

## Architecture Seam

The architecture seam is fixed from day one:

- **Surf Ace controllers**
  - discovery
  - pairing
  - topology intent submission
  - bounded local read projections
  - product-specific tool presentation

- **Surf Ace app clients**
  - ordered topology and lifecycle authority
  - stable pane/surface identity and resolved geometry
  - per-pane visible history
  - protocol operation admission and sequencing
  - render pane content
  - annotations
  - readback
  - normal app mode

- **Surf Ace compositor**
  - Linux-only host/compositor mode
  - backend, device, and output selection
  - output ownership
  - layer placement
  - focus and input routing between fullscreen app and overlay app
  - output rotation
  - hosted native surface lifecycle
  - connector change detection, safe rebinding, and explicit fallback reporting

The compositor is not allowed to become a second topology authority. Surf Ace
client-owned topology is the single source of truth for window and pane state;
controllers submit intent and the compositor realizes client-resolved geometry.
The compositor owns display policy, not pane semantics.

## Content Model Impact

V1 adds one new product-facing content concept:

- **external/native surface**

This is the explicit content type for hosted native applications in compositor mode. It exists to avoid pretending a native terminal or native app is `html`.

V1 rules:

1. Existing Surf Ace content types remain intact for normal app mode and for the Surf Ace main app surface.
2. Any pane in host/compositor mode may render either:
   - a normal Surf Ace-rendered pane content item, or
   - an external/native surface target
3. The active content driver for a pane must be able to switch dynamically at runtime between Surf Ace-rendered content and external/native surface hosting.
4. For a native-hosted pane, the pane payload is an exec or process spec (command + args, optional cwd or env) under Surf Ace client truth.
5. Surf Ace must be able to tell the compositor which app or process spec to run for a pane when that pane switches to an external/native surface.
6. Controllers must still reason in Surf Ace terms:
   - pane or window topology
   - pane identity
   - client-resolved pane geometry
   - current content kind for a pane
   - explicit pane content mode or state for Surf Ace-rendered vs external or native-hosted content
7. Discovery and pairing stay at the Surf Ace app or surface level, not per external app hosted inside a pane.
8. Rotation remains a compositor or output concern, not a controller content concern.

This spec does not fully define the final controller wire schema for `external/native surface`, but it does require that v1 implementation reserve an explicit controller-facing representation of client truth for it rather than reusing `html`.

Minimum v1 requirement for that representation:

- it must be a distinct content kind
- it must identify the requested native target class as `terminal`
- it must leave room for a future target-specific payload without changing the fullscreen or overlay topology model

V1 does not need a generalized external-app schema beyond that minimum contract.

## Recommended Stack

Use a dedicated Rust compositor built on **Smithay**, with:

- Smithay compositor or server stack
- DRM or KMS + GBM for real host mode
- libseat + udev for session and device management
- `winit` backend for early development and local bring-up
- `xdg-shell` support for the Surf Ace main app
- layer placement support sufficient to keep one fullscreen layer plus one overlay layer

Reference influence from upstream projects is intentional:

- **Weston** shows mature DRM backend behavior, explicit logging, output management, and tooling-oriented control surfaces. Useful reference files include `libweston/backend-drm/drm.c` and `frontend/main.c`.
- **gamescope** is a strong reference for automatic-first embedded behavior with explicit escape-hatch flags such as `--backend`, `--prefer-vk-device`, `--prefer-output`, and `--force-orientation`, visible in `src/main.cpp`, and for DRM connector selection policy in `src/drm.cpp`.
- **wlroots** is the reference for reacting to DRM device and session changes by rescanning connectors instead of assuming static output state, as seen in `backend/drm/backend.c`.
- **Cage** is the reference for a narrow kiosk policy shape, auto-enabling outputs, choosing preferred modes, and keeping lifecycle simple rather than shell-like, as seen in `cage.c` and `output.c`.

These are design references, not product templates. Surf Ace remains a Smithay compositor with Surf Ace-specific pane authority.

## Runtime Selection and Override Policy

V1 runtime policy is automatic by default and explicit by exception.

### Automatic default behavior

At startup the compositor should, in order:

1. choose the appropriate runtime class, typically `winit` for local development and DRM or KMS host mode for real output ownership
2. choose the best available device for that runtime
3. discover available outputs on that device
4. choose the active output path and a usable mode
5. expose the chosen path through status and logs

The default path should resemble how Cage relies on `wlr_backend_autocreate(...)` for ordinary backend discovery and how wlroots DRM backends begin by scanning connectors immediately on start.

### Required overrides

V1 must support explicit overrides for:

- backend or runtime class
- preferred device or DRM node
- preferred output connector or connector order
- output rotation
- control socket path for bootstrap and recovery tooling

The operator-facing rule is simple:

- if automatic selection succeeds, operators should not need to think about any of this
- if automatic selection is wrong, there must be a supported way to pin the right choice without rebuilding code or patching the compositor

### Override precedence

1. explicit CLI or environment override
2. persisted local runtime config, if v1 includes one
3. automatic detection

If an override is supplied, status and logs must make that obvious.

### Rejected override behavior

If an override points to a missing device, disconnected connector, or unsupported backend, the compositor must not silently downgrade. It must either:

- fail fast with an explicit error, or
- fall back only if the operator opted into fallback behavior and the fallback is logged explicitly

This follows the good part of gamescope’s model: automatic defaults for ordinary users, but direct operator levers when needed.

## Required Subsystems

V1 requires these subsystems:

### 1. Compositor runtime

- startup and shutdown path
- Smithay event loop integration
- automatic backend selection with explicit override support:
  - `winit` for local development work
  - DRM or KMS path for host mode
- explicit device selection and status reporting for host mode

### 2. Output management

- exactly one active output is enough for v1
- rotation support at the output layer
- layout recomputation for fullscreen plus overlay after rotation
- a tiny direct bootstrap or recovery control path for output rotation and basic output query, usable before Surf Ace is fully operational
- a host-local screen-capture operation on that same control path which writes the current compositor output to an image file for remote inspection or debugging
- automatic output discovery and preferred-mode selection
- active-port detection and safe rebind behavior so a cable move or monitor hotplug can switch host output ownership to the newly active connector instead of requiring manual rediscovery by the user
- explicit operator override hooks for output selection when automatic choice is wrong

This section should follow a blended upstream lesson set:

- from wlroots DRM, treat connector state as dynamic and rescan on session or device change
- from Cage `output.c`, enable outputs and choose preferred modes automatically when possible
- from gamescope, allow explicit connector preference or forcing when the environment is ambiguous
- from Weston DRM, keep the output pipeline observable enough that failures, disconnects, and recovery actions are obvious in logs

#### Screen-capture rules for v1

1. The operation is debug or bootstrap tooling, not a new product topology or authority path.
2. The capture must be invokable by a command on the host machine while the compositor is running.
3. The capture output must be an image file written to a caller-specified filesystem path.
4. The captured image must represent the compositor output as the user sees it on the panel, including the active output rotation, rather than a pre-rotation scene texture or unrelated client-local snapshot.
5. The feature must ride the existing compositor control surface rather than introducing a second debug protocol.
6. It is acceptable for v1 to support only the single active output and to fail closed when no current compositor frame is available yet.

#### Active-port switch rules for v1

1. The compositor must watch connector or port hotplug changes on the host output stack.
2. When the currently active monitor is unplugged and a different connector becomes the new viable path for the same intended display, the compositor should automatically attempt to rebind output ownership to that connector.
3. Safe automatic rebinding means all of the following are true:
   - the compositor still has a valid session and device handle
   - only one viable desktop output remains, or one output is clearly preferred by explicit policy
   - the operation does not require broadening from single-output policy into multi-monitor management
   - the target path can be initialized without leaving the compositor in an ambiguous partially bound state
4. If the compositor cannot safely complete that handoff in-process, it must fail closed with explicit status that the active port changed and a restart or manual recovery is required.
5. The compositor must not silently keep presenting to a stale or disconnected output path while reporting healthy output ownership.
6. The single-output v1 policy still applies, this is active-path handoff, not multi-monitor desktop management.
7. If an explicit output override is in effect, automatic rebinding must respect that policy and may refuse a different connector rather than second-guessing the operator.

#### Output identity policy

The compositor should track output identity using more than the transient connector name when possible, for example connector name plus EDID-derived make or model or other stable metadata. This is the practical lesson behind wanting safe rebinding instead of brittle connector pinning. Connector names such as `HDMI-A-1` or `DP-1` are useful operator handles, but they are not always stable enough to be the only identity primitive.

### 3. Surface role management

- one fullscreen app role
- one overlay app role
- fixed z-order
- no general-purpose tiling, floating, or window-manager behavior

### 4. App launch and attachment

- launch Surf Ace Linux app as the fullscreen app
- launch a Wayland-native terminal or CLI app as the overlay app
- bind each client into the correct role deterministically
- enforce one compositor-managed surface slot per pane-hosted native app in v1
- allow dependent transient child surfaces to remain attached to the owning pane
- deny or explicitly mark unsupported any additional independent top-level window behavior

### 5. Input and focus routing

- explicit active target between fullscreen app and overlay app
- predictable keyboard focus for the terminal overlay
- pointer routing that matches visible layer ownership
- explicit ownership rules for focus, input, selection, and annotation when a pane switches between Surf Ace-rendered content and external or native hosting

### 6. Controller-facing bridge seam

- compositor host mode must expose enough applied-state truth for Surf Ace clients and controller projections to stay coherent
- explicit representation for hosted native overlay content
- no fake `html` wrapper for the native overlay target
- external or native hosting is a pane content mode under Surf Ace authority, not a peer authority
- explicit client-resolved pane mode or state for Surf Ace-rendered vs external or native-hosted panes
- Surf Ace should call into the compositor through the same underlying control surface used for bootstrap or recovery operations once Surf Ace is running

For v1, the controller-facing projection of the client/compositor bridge only needs to answer these questions:

- is host or compositor mode active
- which panes are currently Surf Ace-rendered vs external or native-surface hosted
- for an external or native pane, what exec or process spec was requested
- is the external or native target absent, launching, attached, failed, or exited
- what output rotation is active
- what runtime, device, and output were auto-selected or explicitly forced
- which event families are supported, adapted, or suppressed for external or native panes

## Logging, Status, and Fallback Semantics

Fallback is acceptable in v1. Silent fallback is not.

The compositor must emit explicit logs and status for these classes of event:

- runtime or backend auto-selection
- device auto-selection
- output auto-selection
- explicit override use
- rejected override use
- fallback to a less preferred backend, device, mode, or output
- connector loss, connector rebinding, or recovery failure
- forced degraded modes such as software paths, readback-forced paths, or no-output-yet conditions

Each such record should answer, in plain language:

1. what the compositor wanted to use
2. what it actually used
3. whether the result was automatic, forced, or fallback behavior
4. why that result happened
5. whether operator intervention is needed

Examples of the desired style:

- `auto-selected backend=drm device=/dev/dri/card1 output=HDMI-A-1 mode=1080x1920@60`
- `forced output override DP-1 rejected: connector not present`
- `active connector HDMI-A-1 disappeared, rebound to HDMI-A-2 using matching display identity`
- `fallback to winit backend: DRM device acquisition failed`
- `recovery required: previous output disappeared and no single safe replacement was found`

Weston’s `frontend/main.c` and DRM backend logging are the model for taking observability seriously. wlroots DRM connector rescans and Cage output lifecycle handling are the model for logging state transitions as part of normal operation rather than as rare debug-only events.

## First Product Slice Scope

The first product slice is intentionally narrow and exists to prove the long-term pane-hosting seam without pretending the product is limited to one overlay forever.

### In scope

- Smithay compositor booting in `winit`
- same compositor booting in DRM or KMS host mode
- one fullscreen Surf Ace app
- one overlay terminal or CLI app
- fixed overlay placement
- output rotation
- automatic backend, device, and output selection with explicit override escape hatches
- connector rebinding behavior for ordinary single-display hotplug moves
- explicit internal notion of external or native overlay content
- explicit fallback and recovery logging

### Out of scope

- multiple overlay apps
- multiple fullscreen apps
- dynamic layout composition
- Xwayland
- arbitrary native app catalog or registry
- generalized provider UX for selecting among many external app types
- annotation semantics inside the hosted terminal app
- broad multi-monitor desktop management

## Milestone Slices

Implementation should proceed in these slices:

### Slice 1: Local compositor bring-up

Goal:
- prove the compositor runtime and architecture seam without host-mode complexity

Deliver:
- Smithay compositor running on `winit`
- Surf Ace app can appear as the fullscreen client
- one simple overlay client can appear above it
- startup status reports that `winit` was selected automatically or forced explicitly

Exit check:
- two-layer stack visible in local development backend

### Slice 2: Fixed-role window policy

Goal:
- make the fullscreen-vs-overlay policy explicit and deterministic

Deliver:
- fullscreen role assignment
- overlay role assignment
- fixed z-order
- basic input or focus switching
- Surf Ace main app runs as the fullscreen client under the compositor
- host-mode detection path so Surf Ace knows it is running under the compositor

Exit check:
- Surf Ace main app is always fullscreen
- overlay app is always above it
- Surf Ace can detect compositor host mode

### Slice 3: Dynamic pane-hosting bridge

Goal:
- prove the per-pane driver switch seam before broad native-app scope

Deliver:
- a client-facing bridge where a client-committed pane can switch live between Surf Ace-rendered content and external or native surface hosting
- pane geometry remains owned and resolved by the Surf Ace client
- the compositor can receive which external app target to run for a pane
- reversible switching back to Surf Ace-rendered content without redefining the pane
- explicit pane mode or state in Surf Ace client truth for Surf Ace-rendered vs external or native-hosted panes
- explicit reduced or adapted event contract for external or native panes

Exit check:
- at least one pane can switch from Surf Ace-rendered content to an external or native target and back at runtime
- that pane remains under Surf Ace topology authority the whole time

### Slice 4: Terminal overlay product slice

Goal:
- prove the first honest native hosted target

Deliver:
- launch a Wayland-native terminal or CLI app as the overlay
- wire enough lifecycle handling for attach, detach, and restart
- name the controller-facing projection as external or native overlay content
- support a small state machine for the overlay target:
  - absent
  - launching
  - attached
  - failed

Exit check:
- terminal overlay can be launched, focused, and dismissed without disturbing the fullscreen Surf Ace app

### Slice 5: Host/compositor mode

Goal:
- move from local bring-up to real Linux host mode

Deliver:
- DRM or KMS + GBM path
- libseat + udev device or session handling
- automatic device discovery and output selection
- same fixed fullscreen plus overlay policy under real output ownership
- explicit backend, device, and output status reporting

Exit check:
- compositor runs on a Linux host with real output ownership
- logs show which runtime, device, and connector were chosen and why

### Slice 6: Rotation and bootstrap control

Goal:
- make host mode usable on rotated displays from day one

Deliver:
- output rotation support
- fullscreen and overlay layout recomputed correctly after rotation
- input coordinates remain correct after rotation
- working local control path for query, rotate, and capture operations

Exit check:
- rotated output still preserves the expected main or overlay arrangement
- remote operator can query and rotate without depending on Surf Ace UI readiness

### Slice 7: Connector move and fallback behavior

Goal:
- make single-display host mode survive ordinary cabling or port moves without feeling brittle

Deliver:
- connector or hotplug detection
- safe automatic rebinding to the new active connector when unambiguous
- explicit recovery-required status when rebinding is unsafe or ambiguous
- clear logs for auto-rebind, forced pinning, fallback, and failure cases

Exit check:
- moving the monitor to a different viable connector either recovers automatically or produces a precise recovery-required state, never a silent false-healthy state

## Implementation Boundaries

To preserve the right long-term seam:

1. Do not put compositor logic into Electron.
2. Do not make the compositor a Surf Ace controller or second topology authority.
3. Do not let hosted native apps enter the system as fake `html`.
4. Do not widen v1 into app-compat or platform breadth before the terminal overlay path is honest and working.
5. Do not change normal app mode behavior in order to make host mode work.
6. Do not make explicit overrides the normal operator path when automatic detection can solve the problem reliably.
7. Do not hide fallback decisions inside opaque heuristics, every meaningful fallback must be observable.

## Upstream Reference Notes

These references are included to anchor implementation choices, not to mandate line-by-line copying.

### Weston

- `libweston/backend-drm/drm.c` is the reference for serious DRM output lifecycle handling, disconnect awareness, output-capture-adjacent plumbing, and explicit failure logging.
- `frontend/main.c` is the reference for a compositor that treats logging, backend configuration, and child-process lifecycle as first-class operational concerns.

### gamescope

- `src/main.cpp` is the clearest upstream example of automatic defaults plus explicit operator escape hatches, including `--backend`, `--prefer-vk-device`, `--prefer-output`, and orientation forcing.
- `src/drm.cpp` is the reference for connector-focused DRM policy, output naming, and explicit connector preference logic in a single-purpose embedded compositor.

### wlroots

- `backend/drm/backend.c` is the key reference for rescanning DRM connectors on backend start, session resume, and device change instead of assuming the output set is static.
- This is the strongest upstream lesson behind Surf Ace’s required active-port rebind behavior.

### Cage

- `cage.c` shows the value of auto-created backend selection for kiosk behavior and a narrow primary-client lifecycle.
- `output.c` shows the value of auto-enabling outputs, preferring a working mode, updating output layout centrally, and keeping output handling simple enough to reason about operationally.

## Verified operator workflow on RACTER tty4

- Canonical repo: `/home/clu/src/surf-ace-compositor`
- Verified host runtime socket: `/tmp/surf-ace-zsh-tty4.sock`
- Verified main app for the live verification: Ghostty running `zsh`
- Verified present path: `direct_gbm`
- `SURF_ACE_HOST_RUNTIME_FORCE_READBACK` remains unset for the normal verified workflow

Run the compositor binary in host mode from tty4:

```bash
cd /home/clu/src/surf-ace-compositor
source ~/.cargo/env >/dev/null 2>&1
cargo build
sudo chvt 4
sudo bash -lc "setsid bash -lc 'exec </dev/tty4 >/tmp/surf-ace-compositor-root-tty4.log 2>&1; cd /home/clu/src/surf-ace-compositor; source ~/.cargo/env >/dev/null 2>&1; exec env XDG_RUNTIME_DIR=/run/user/1000 LIBSEAT_BACKEND=seatd ./target/debug/surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock' &"
```

The live recovery contract is the current binary or help surface above:
`serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock`. The older
`serve --tty /dev/tty4` form is removed and any note that still uses it is stale.

Launch the verified terminal app against that compositor:

```bash
sudo env XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 \
  ghostty --gtk-single-instance=false -e zsh -i
```

Control rotation over the compositor socket:

```bash
sudo ./target/debug/surf-ace-compositor rotate \
  --socket-path /tmp/surf-ace-zsh-tty4.sock \
  --rotation deg90
```

The verified rotation values on RACTER tty4 are `deg0`, `deg90`, `deg180`, and `deg270`.

Capture the current panel-equivalent compositor output over that same socket:

```bash
sudo ./target/debug/surf-ace-compositor capture \
  --socket-path /tmp/surf-ace-zsh-tty4.sock \
  --output-path /tmp/surf-ace-capture.png
```

What is now verified on RACTER tty4:

- the host runtime starts from the standalone compositor binary rather than Electron
- runtime control happens over the local Unix socket control path
- rotation control happens over that same control path
- `capture_screen` writes a PNG representing the live panel view
- Ghostty plus interactive `zsh` stays live on the real `direct_gbm` path
- captures are verified readable and correctly oriented for `deg0`, `deg90`, `deg180`, and `deg270`

## Deferred Work

These are explicitly deferred beyond v1:

- Xwayland
- multiple overlay layers
- multiple displays as a product feature
- generalized native app launcher or registry
- arbitrary overlay positioning rules
- provider UX for selecting many external app classes
- app-specific readback semantics for hosted native surfaces
- richer native surface types beyond the terminal or CLI initial target
- cross-platform compositor ambitions outside Linux

## Risks

### Risk 1: Host-mode complexity swallows the current v1 slice

Mitigation:
- require `winit` bring-up first
- do not start with DRM or KMS as the first proof point

### Risk 2: Native overlay scope balloons into general desktop support

Mitigation:
- lock v1 to one Wayland-native terminal or CLI overlay target
- explicitly defer Xwayland and general app compatibility

### Risk 3: Compositor becomes a second authority

Mitigation:
- keep Surf Ace client topology as the single source of truth
- accept only client-resolved pane identity and geometry, never controller topology intent
- constrain compositor responsibilities to display policy, input routing, and native surface hosting

### Risk 4: Automatic selection becomes brittle magic

Mitigation:
- keep auto-selection policy narrow and legible
- add explicit operator escape hatches for backend, device, and output
- make every fallback and recovery path visible in logs and status

### Risk 5: Connector rebinding becomes accidental multi-monitor policy

Mitigation:
- define rebinding strictly as single-active-output recovery
- refuse ambiguous scenarios instead of improvising desktop-like behavior
- surface recovery-required state explicitly when the right answer is not obvious

## Remaining Implementation Questions

These do not block the v1 spec, but they still need concrete implementation answers:

1. What is the exact provider or compositor wire shape for the first `external/native surface` representation?
2. For v1 app selection, do we start with a fixed named target (`terminal`) and then widen later, or immediately expose the fuller exec or process contract at the controller-facing layer?
3. What is the smallest concrete control-surface implementation for bootstrap or recovery operations, for example Unix socket, localhost RPC, or tiny CLI wrapper over the same local API?
4. What exact identity tuple should Surf Ace treat as the preferred single-display identity for safe port rebinding, connector name alone, connector plus EDID tuple, or connector plus stable display metadata cache?
5. In ambiguous hotplug situations, should the compositor hold the last good forced output policy indefinitely, or should it expose a timed recovery-required state that invites operator choice?

## Implementation Handoff

Engineers can start from this spec if they preserve these boundaries:

- build a separate Rust compositor project on Smithay
- prove the local `winit` path first
- keep normal Surf Ace Linux app mode unchanged
- keep protocol admission and topology authority in the Surf Ace client; controllers submit intent
- treat the terminal overlay as the first real external or native surface target, not as a placeholder `html` path
- make backend, device, and output selection automatic by default, with explicit overrides and explicit fallback logging
- implement connector move handling as safe single-output rebinding, not as a quiet expansion into desktop window management

If implementation pressure pushes toward Electron-as-compositor, fake-HTML native hosting, override-only operations, silent output fallback, or broad Xwayland or app-compat scope, that is a spec violation rather than a reasonable shortcut.
