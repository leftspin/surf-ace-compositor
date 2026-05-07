# Surf Ace Compositor V1 Spec

## Goal

Define the first implementation-ready Linux compositor mode for Surf Ace without changing Surf Ace’s normal app-mode product shape on existing desktops.

The chosen direction is fixed for this spec:

- Rust + Smithay
- normal Surf Ace app mode remains unchanged
- Linux gets an optional host/compositor mode
- Electron is not the compositor
- v1 host mode must be able to run Surf Ace itself as the main app inside the compositor
- v1 host mode must accept an explicit startup parameter for the fullscreen/main app launch intent; that intent is an exact process spec plus explicit expected surface binding identity, not a loose match hint
- the same fullscreen/main app launch-selection action must also be available on the compositor control surface, including when that control surface is reached over the network
- v1 host mode is one fullscreen main app plus one overlay app layer for the first product slice, but the long-term pane-hosting model is per-pane and dynamic
- output rotation is supported in host mode
- the first honest external-surface target is a terminal/CLI app
- hosted native apps become an explicit external/native surface content type instead of being modeled as `html`
- Surf Ace must be able to detect when it is running under the compositor host mode
- Surf Ace must be able to tell the compositor which app to run in a pane, and switch that pane dynamically between Surf Ace-rendered content and external/native surface hosting
- local development runtime (`winit`) and true host compositor runtime (DRM/KMS) are distinct runtime modes and must not be silently collapsed

## Product Shape

Surf Ace continues to have two product modes on Linux:

1. **Normal app mode**
   - Surf Ace runs as a normal app inside any installed compositor or desktop session.
   - This mode is unchanged by the compositor project.

2. **Optional host/compositor mode**
   - Surf Ace runs with a dedicated Linux compositor process that owns the output stack.
   - This mode is Linux-only and optional.

## V1 Overlay Policy vs Long-Term Contract

These two truths must not be collapsed into each other:

- **First product-slice policy**
  - one fullscreen main app role, with Surf Ace as a required supported target
  - one overlay app layer
  - terminal/CLI as the first honest native-hosted target
  - overlay placement remains fixed compositor policy relative to the active output; the current v1 slice does not yet realize provider pane geometry for native overlays

- **Long-term product contract**
  - any pane may dynamically switch between Surf Ace-rendered content and native-hosted content
  - native-hosted content remains under Surf Ace/provider pane authority
  - the compositor realizes native surfaces inside Surf Ace-defined pane rectangles

The v1 overlay policy is a narrow proving slice. It is not the long-term limit of the pane-hosting model.

## Spec Invariants

These invariants are required even if implementation details change:

1. Surf Ace must be able to run as the main app inside the compositor, even though it is an Electron app, and the fullscreen/main app to start must be selected by an explicit startup parameter rather than by a loose match hint.
2. Electron may be the main client, but it may not become the compositor.
3. Surf Ace must be able to detect that it is running under Surf Ace compositor host mode.
4. Pane geometry and pane identity remain Surf Ace/provider truth; the compositor must not become a second topology authority.
5. Any pane may dynamically switch between Surf Ace-rendered content and an external/native surface.
6. Surf Ace must be able to tell the compositor which app to run in a pane when that pane switches to an external/native surface.
7. For native-hosted panes, the payload primitive is an executable/process spec supplied under Surf Ace authority (command + args, with optional cwd/env), not a separate app/surface authority model.
8. The switch between Surf Ace-rendered content and external/native surface hosting must be live and reversible at runtime, not one-time startup configuration.
9. External/native surfaces must be represented explicitly, never as `html`.
10. Rotation remains a compositor/output concern, not a provider content concern.
11. The first product slice may use a single terminal/CLI target, but the pane-hosting abstraction must already support future standard Linux GUI apps.
12. For tmux/terminal targets, persistent session state may survive outside the pane binding even when the native surface attachment is destroyed and recreated.
13. External/native hosting is a pane content mode under Surf Ace authority, not a peer authority.
14. Discovery and pairing remain attached to the Surf Ace app/surface instance, not to hosted child apps inside panes.
15. External/native panes must have an explicit reduced or adapted event contract rather than inheriting HTML-centric event semantics by accident.
16. Focus, input, selection, and annotation ownership must be explicit whenever a pane switches between Surf Ace-rendered and external/native content.
17. A pane-hosted native app gets one compositor-managed surface slot in v1.
18. Transient child surfaces (menus, tooltips, true dialogs/popups) may remain attached to that pane if they are dependent on the hosted app surface.
19. Attached transient child surfaces may visually float above pane content, but they remain owned by that pane and must be clipped/repositioned within pane policy rather than escaping into global desktop space.
20. Independent additional top-level windows must not silently escape into free-floating window-manager behavior in v1; they must be denied, collapsed into pane policy, or treated as unsupported.
21. The compositor must expose a tiny direct bootstrap/control path for setup and recovery operations such as output rotation, so sideways-monitor bring-up does not depend on Surf Ace already being usable.
22. Surf Ace should use that same compositor control path once running; that control surface must also expose the same primary/fullscreen app launch-selection action after startup, and any network exposure must remain a thin wrapper over the same underlying control surface rather than a second authority path.
23. Fullscreen-main vs overlay-native role binding must be deterministic from explicit policy/identity signals, not from incidental client connection order; for the main role that means an exact executable/process spec (exact command/path or equivalent explicit process descriptor) plus explicit expected surface binding semantics.
24. Overlay-native role admission must remain subordinate to pane content-mode/lifecycle truth; overlay role attach/detach must reconcile with that pane’s external/native lifecycle state.
25. Host compositor runtime must fail closed if no seat-managed DRM device can be acquired; it must not silently fall back to development backend behavior.
26. The compositor must define a default keyboard shortcut that toggles the shell/terminal overlay open and closed; the default is Super+`, that shortcut must be overridable from the compositor command line at startup, and shortcut-triggered open/dismiss transitions must keep focus/input ownership coherent between the fullscreen app and the overlay lifecycle.
27. Once host runtime selection has begun, later host failure remains a host-mode failure; it must not be reinterpreted as a silent backend reselection or automatic downgrade to `winit`.
28. Post-selection host failure must preserve the compositor’s local control path for status, rotation, and explicit recovery; recovery stays on that same underlying control surface rather than creating a second authority path.
29. In the first fullscreen-plus-overlay product slice, fixed overlay placement is compositor policy derived from the active output; role/lifecycle signals from that product slice must not be mistaken for completion of provider-defined pane-rectangle realization.
30. Overlay lifecycle truth must distinguish requested or launching state from attached state; shortcut-open intent or pending process launch is not itself an attached overlay-native role binding.

## V1 Success Criteria

V1 is successful when all of the following are true:

1. A dedicated Surf Ace compositor can run in a local development backend and in real host/compositor mode on Linux.
2. The compositor can show exactly one fullscreen main app and one overlay app layer at the same time.
3. The compositor can rotate the output while preserving the expected fullscreen/overlay arrangement.
4. The first overlay target is a Wayland-native terminal/CLI app.
5. The provider-facing model remains recognizably Surf Ace:
   - provider still targets windows/panes/topology the same way
   - compositor policy does not become a second provider
   - hosted native overlay content is represented explicitly, not smuggled through `html`
6. Runtime role behavior is deterministic and pane-authoritative:
   - main-role launch-selection is available both at startup and on the control surface, including the network-reached form of that same control surface
   - main-vs-overlay role binding is not dependent on incidental client ordering
   - main-role attachment is reported only after an arriving surface matches the declared primary launch/binding contract
   - overlay-native role admission remains subordinate to pane external/native state
7. Shell-overlay access is compositor-owned and startup-configurable:
   - the default keyboard shortcut is Super+` and toggles the shell/terminal overlay open and closed
   - the default shortcut can be overridden on the compositor command line at startup
   - shortcut-triggered open/dismiss transitions keep keyboard focus and input routing coherent
8. Host backend readiness is explicit:
   - host mode activates only after seat-managed DRM device acquisition succeeds and real output ownership is live
   - acquisition failure is explicit and does not silently downgrade to development backend behavior
   - once host-mode start has begun, later host failure remains failed host mode rather than silently downgrading to `winit`
   - the local control path remains reachable after host failure for status and explicit recovery
9. Overlay role status remains honest about what is and is not complete:
   - fixed overlay placement is compositor policy for the current v1 slice, not proof that provider pane-geometry realization is already complete
   - overlay-bound role status is reported only for a concretely attached native surface, not merely for shortcut-open or launch intent

## Non-Goals

This spec does not include:

- replacing normal Surf Ace app mode on Linux
- making Electron the compositor
- general desktop shell features
- multi-overlay stacking
- arbitrary window management
- Xwayland support
- native app embedding inside Electron
- provider-originated compositor control beyond what is needed to select main content vs overlay content
- generalized app sandboxing or app-store style policy
- a cross-platform compositor abstraction for macOS or iPad

## Architecture Seam

The architecture seam is fixed from day one:

- **OpenClaw/provider**
  - discovery
  - pairing
  - topology authority
  - per-pane visible history
  - protocol and operation routing

- **Surf Ace app clients**
  - render pane content
  - annotations
  - readback
  - normal app mode

- **Surf Ace compositor**
  - Linux-only host/compositor mode
  - output ownership
  - layer placement
  - focus/input routing between fullscreen app and overlay app
  - output rotation
  - hosted native surface lifecycle

The compositor is not allowed to become a second topology authority. Provider-owned topology remains the single source of truth for Surf Ace window/pane state. The compositor owns display policy, not pane semantics.

That same boundary applies to the shell/terminal overlay toggle shortcut: default binding, startup CLI override, and resulting open/focus/dismiss behavior are compositor concerns, not provider topology authority.

That same boundary also applies to the fullscreen/main role. The compositor may own the launch-and-attach policy for the primary app, but that policy must be expressed as an explicit contract: exact process spec plus explicit expected surface binding identity, with the same action available at startup and through the control surface.

That same v1 boundary also applies to overlay placement and status. In the first fullscreen-plus-overlay slice, the compositor uses fixed overlay placement relative to the active output in order to prove lifecycle, focus, and role ownership. Those v1 role/lifecycle signals must not be read as evidence that the long-term provider-defined pane-rectangle realization contract is already complete.

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
4. For a native-hosted pane, the pane payload is an exec/process spec (command + args, optional cwd/env) under Surf Ace/provider truth.
5. Surf Ace must be able to tell the compositor which app/process spec to run for a pane when that pane switches to an external/native surface.
6. The provider must still reason in Surf Ace terms:
   - pane/window topology
   - pane identity
   - pane geometry ownership
   - current content kind for a pane
   - explicit pane content mode/state for Surf Ace-rendered vs external/native-hosted content
7. Discovery and pairing stay at the Surf Ace app/surface level, not per external app hosted inside a pane.
8. Rotation remains a compositor/output concern, not a provider content concern.
9. In the first overlay product slice, fixed native-overlay placement remains compositor output policy; it does not transfer pane geometry authority away from Surf Ace/provider or mean pane rectangles already drive native placement.

This spec does not fully define the final provider wire schema for `external/native surface`, but it does require that v1 implementation reserve an explicit provider-facing representation for it rather than reusing `html`.

Minimum v1 requirement for that representation:

- it must be a distinct content kind
- it must identify the requested native target class as `terminal`
- it must leave room for a future target-specific payload without changing the fullscreen/overlay topology model

The compositor control API may accept a provider-authored native pane host plan
containing pane identity, pane rectangle, target class, and process intent for
more than one pane at a time. That API is a mapping/configuration primitive: Surf
Ace remains the source of pane geometry and content-mode truth, while the
compositor records and later realizes those native surface slots. The compositor
must not derive new panes or reinterpret provider geometry as its own layout
model.

Launching native pane hosts is a separate lifecycle transition from recording the
plan. Surf Ace can ask the compositor to launch a selected set of recorded native
pane hosts, or all recorded native pane hosts, and each pane reports its own
`absent`/`launching`/`attached`/`failed`/`exited` lifecycle. Launching a native
pane host must carry the Surf Ace pane identity into the process environment so
later surface binding and status reconciliation can stay pane-scoped.

V1 does not need a generalized external-app schema beyond that minimum contract.

## Recommended Stack

Use a dedicated Rust compositor built on **Smithay**, with:

- Smithay compositor/server stack
- DRM/KMS + GBM for real host mode
- libseat + udev for session/device management
- winit backend for early development and local bring-up
- `xdg-shell` support for the Surf Ace main app
- layer placement support sufficient to keep one fullscreen layer plus one overlay layer

Reference influence from Cage is acceptable for the single-app policy shape, but Cage/Weston are not the product compositor.

## Required Subsystems

V1 requires these subsystems:

### 1. Compositor runtime

- startup/shutdown path
- Smithay event loop integration
- backend selection:
  - `winit` for local development work
  - DRM/KMS path for host mode
- explicit mode boundary:
  - `winit` and host/DRM paths are separate runtime modes with separate readiness criteria
  - host-mode startup failure must be explicit (no transparent fallback to `winit`)
- host preflight must include session acquisition, seat-scoped DRM discovery, and real device acquisition before host mode is considered active
- preflight-only device/session success is not enough to claim running host mode; host-ready/running truth requires real output ownership
- once host-runtime selection/start has begun, later host failure must remain explicit failed host mode rather than collapsing back into backend selection
- post-selection host failure must keep the local control path alive for status and explicit recovery on the same underlying control surface

### 2. Output management

- exactly one output is enough for v1
- rotation support at the output layer
- layout recomputation for fullscreen + overlay after rotation
- a tiny direct bootstrap/recovery control path for output rotation and basic output query, usable before Surf Ace is fully operational
- active host route identity must be observable over control/status at least at the level of render path, scanout path, connector, mode, and current fallback/present state
- route changes must rebuild route-local capability assumptions instead of reusing stale direct-scanout or overlay-path optimism from a previous connector/card path
- if direct or overlay presentation cannot truthfully realize the requested route/rotation, the compositor must stay alive and drop to a lower truthful presentation path instead of claiming stale or partial success

### 3. Surface role management

- one fullscreen app role
- one overlay app role
- fixed z-order
- no general-purpose tiling/floating/window manager behavior
- deterministic main/overlay binding policy:
  - main role binding is based on an explicit launch-and-binding contract, not first-come fallback or loose client matching
  - overlay role is admitted only when pane-level external/native state expects it

### 4. App launch and attachment

- accept an explicit startup parameter for the fullscreen/main app launch intent
- accept the same fullscreen/main app launch-selection action on the compositor control surface, including when that control surface is reached over the network
- require the fullscreen/main app launch intent to name an exact executable/process spec (exact command/path or equivalent explicit process descriptor)
- require the fullscreen/main app launch intent to declare the expected surface identity/binding semantics for deterministic attachment when the Wayland surface appears
- Surf Ace Linux app must be a supported fullscreen/main app target
- launch a Wayland-native terminal/CLI app as the overlay app
- bind each client into the correct role deterministically
- reconcile the fullscreen/main surface to the expected primary launch/binding contract before reporting the main role as attached/bound
- reconcile an admitted native overlay surface to the expected pane/process request before reporting that pane as attached/bound
- keep overlay-bound status absent while the requested overlay is only reserved, launching, pending identity, failed, or exited
- enforce one compositor-managed surface slot per pane-hosted native app in v1
- allow dependent transient child surfaces to remain attached to the owning pane
- transient child surfaces must be accepted only when parent ownership maps to a known pane-bound role, and must remain constrained by pane policy
- deny or explicitly mark unsupported any additional independent top-level window behavior

### 5. Input/focus routing

- explicit active target between fullscreen app and overlay app
- predictable keyboard focus for the terminal overlay
- compositor-owned default keyboard shortcut (Super+`) that toggles the shell/terminal overlay open and closed
- startup command-line configuration for that shortcut without creating a second provider/topology authority
- shortcut-triggered overlay open gives keyboard focus to the overlay when the overlay becomes active
- shortcut-triggered overlay dismiss returns focus/input routing to the fullscreen app or other non-overlay active target coherently
- pointer routing that matches visible layer ownership
- explicit ownership rules for focus, input, selection, and annotation when a pane switches between Surf Ace-rendered content and external/native hosting
- transient/popup focus and pointer routing must resolve through owning role/pane policy, not as independent desktop windows

### 6. Provider-facing bridge seam

- compositor host mode must expose enough state for the existing Surf Ace provider model to stay coherent
- explicit representation for hosted native overlay content
- no fake `html` wrapper for the native overlay target
- external/native hosting is a pane content mode under Surf Ace authority, not a peer authority
- explicit pane mode/state for Surf Ace-rendered vs external/native-hosted panes
- Surf Ace should call into the compositor through the same underlying control surface used for bootstrap/recovery operations once Surf Ace is running
- runtime role lifecycle and runtime status must stay reconcilable with pane lifecycle truth and host-backend readiness
- runtime status for the fullscreen/main role must distinguish requested launch intent from concrete attachment
- overlay role lifecycle status must distinguish launch intent from concrete attachment; any overlay-bound-pane signal is truthful only for attached state and must clear again on failure or exit
- v1 fullscreen/overlay status proves compositor role/lifecycle truth only; it does not claim that provider-pane geometry already drives overlay placement

For v1, the provider-facing bridge only needs to answer these questions:

- is host/compositor mode active
- what fullscreen/main launch intent is currently requested
- which client (if any) currently owns the attached fullscreen/main role
- which panes are currently Surf Ace-rendered vs external/native-surface hosted
- for an external/native pane, what exec/process spec was requested
- is the external/native target absent, launching, attached, failed, or exited
- what output rotation is active
- which event families are supported, adapted, or suppressed for external/native panes
- which pane (if any) currently owns an attached overlay-native role binding
- whether host backend readiness has acquired seat-scoped DRM device ownership

For the first product slice, overlay role/binding answers are about concrete native attachment and lifecycle truth, not about shortcut-open intent, reserved launch state, or completion of provider-pane geometry realization.

### 7. Control/recovery/status truth

- one direct compositor control surface is the authority path for bootstrap, status, rotation, and explicit recovery operations
- once host runtime selection/start has begun, a later host failure must remain queryable and recoverable on that same control surface
- the control surface must preserve single-owner semantics for a given machine/output; parallel host compositors are not a valid recovery mechanism
- the control surface must accept the same explicit fullscreen/main launch-selection action that exists at compositor startup
- status must distinguish:
  - configured/requested runtime intent
  - configured/requested fullscreen/main launch intent
  - active host route ownership/readiness truth
  - actual fullscreen/main attachment truth
  - current present/fallback truth
  - actual overlay attachment truth
- startup configuration, control-surface request acceptance, capability, shortcut intent, reservation, and launch-in-progress must not be reported as equivalent to successful attachment or successful queued presentation
- fail-closed transitions must clear stale route, present, and overlay-binding success claims

## First Product Slice Scope

The first product slice is intentionally narrow and exists to prove the long-term pane-hosting seam without pretending the product is limited to one overlay forever.

### In scope

- Smithay compositor booting in `winit`
- same compositor booting in DRM/KMS host mode
- one fullscreen main app role, with Surf Ace as a required supported target
- one overlay terminal/CLI app
- fixed overlay placement derived from the active output; provider pane geometry is not yet the current v1 placement driver
- output rotation
- explicit internal notion of external/native overlay content
- explicit startup and control-surface selection of the fullscreen/main app
- default compositor keyboard shortcut for toggling the shell overlay open/closed is Super+`, configurable at startup

### Out of scope

- multiple overlay apps
- multiple fullscreen apps
- dynamic layout composition
- Xwayland
- arbitrary native app catalog/registry
- generalized provider UX for selecting among many external app types
- annotation semantics inside the hosted terminal app
- treating v1 fixed overlay placement as equivalent to final provider-pane-rectangle realization

## Milestone Slices

Implementation should proceed in these slices:

### Slice 1: Local compositor bring-up

Goal:
- prove the compositor runtime and architecture seam without host-mode complexity

Deliver:
- Smithay compositor running on `winit`
- Surf Ace app can appear as the fullscreen client
- one simple overlay client can appear above it

Exit check:
- two-layer stack visible in local development backend

### Slice 2: Fixed-role window policy

Goal:
- make the fullscreen-vs-overlay policy explicit and deterministic

Deliver:
- fullscreen role assignment
- overlay role assignment
- fixed z-order
- basic input/focus switching
- explicit startup parameter for fullscreen/main app launch intent
- Surf Ace main app runs as a supported fullscreen client under the compositor
- host-mode detection path so Surf Ace knows it is running under the compositor
- deterministic role policy (main binding + pane-authoritative overlay admission)
- deterministic fullscreen attachment from exact process spec plus declared surface binding identity

Exit check:
- the requested primary app starts in fullscreen from the explicit startup parameter
- fullscreen role attachment occurs only for the surface that matches the declared primary launch/binding contract
- overlay app is always above it
- Surf Ace can detect compositor host mode

### Slice 3: Dynamic pane-hosting bridge

Goal:
- prove the per-pane driver switch seam before broad native-app scope

Deliver:
- a provider/app-facing bridge where a pane can switch live between Surf Ace-rendered content and external/native surface hosting
- pane geometry remains owned by Surf Ace/provider
- the compositor can receive which external app target to run for a pane
- reversible switching back to Surf Ace-rendered content without redefining the pane
- explicit pane mode/state in provider truth for Surf Ace-rendered vs external/native-hosted panes
- explicit reduced/adapted event contract for external/native panes

Exit check:
- at least one pane can switch from Surf Ace-rendered content to an external/native target and back at runtime
- that pane remains under Surf Ace topology authority the whole time

### Slice 4: Terminal overlay product slice

Goal:
- prove the first honest native hosted target

Deliver:
- launch a Wayland-native terminal/CLI app as the overlay
- wire enough lifecycle handling for attach/detach/restart
- name the provider-facing concept as external/native overlay content
- keep fixed overlay placement explicit as compositor output policy for the current v1 slice rather than final pane-rectangle realization
- provide a compositor-owned default keyboard shortcut of Super+` that toggles the shell/terminal overlay open and closed
- allow that default shortcut to be overridden from the compositor command line at startup
- report overlay-bound attachment only after the admitted surface matches the expected pane/process request
- support a small state machine for the overlay target:
  - absent
  - launching
  - attached
  - failed
  - exited

Exit check:
- terminal overlay can be launched, focused, dismissed, and re-opened by the compositor shortcut without disturbing the fullscreen main app
- shortcut-triggered open/dismiss keeps focus/input routing coherent
- overlay-bound status stays absent while the overlay is only reserved, launching, failed, or exited

### Slice 5: Host/compositor mode

Goal:
- move from local bring-up to real Linux host mode

Deliver:
- DRM/KMS + GBM path
- libseat + udev device/session handling
- same fixed fullscreen + overlay policy under real output ownership
- fail-closed host readiness (mode activates only after seat-scoped DRM acquisition and exposes readiness state)
- post-selection host failure remains explicit failed host mode with control-surface recovery, not hidden backend reselection
- the same explicit fullscreen/main app launch-selection action is exposed on the control surface for remote/networked control without creating a second authority path
- control/status exposes active route identity and current present/fallback truth honestly enough for operator recovery and seam validation

Exit check:
- compositor runs on a Linux host with real output ownership
- the operator can issue the same explicit fullscreen/main app launch-selection request over the control surface after startup
- host failure remains queryable and explicitly recoverable on the same control surface

### Slice 6: Rotation

Goal:
- make host mode usable on rotated displays from day one

Deliver:
- output rotation support
- fullscreen and overlay layout recomputed correctly after rotation
- input coordinates remain correct after rotation
- unsupported direct/overlay rotation paths fail closed into a truthful lower presentation path instead of partial-output optimism

Exit check:
- rotated output still preserves the expected main/overlay arrangement

## Implementation Boundaries

To preserve the right long-term seam:

1. Do not put compositor logic into Electron.
2. Do not make the compositor a second Surf Ace provider.
3. Do not let hosted native apps enter the system as fake `html`.
4. Do not widen v1 into app-compat/platform breadth before the terminal overlay path is honest and working.
5. Do not change normal app mode behavior in order to make host mode work.
6. Do not describe v1 fixed overlay placement as if provider pane geometry were already driving native-surface realization.
7. Do not overload overlay-bound/runtime-status fields so they blur together shortcut intent, reservation, and actual native-surface attachment.
8. Do not treat loose client matching or client arrival order as sufficient authority for the fullscreen/main role when an explicit primary launch-and-binding contract exists.

## Deferred Work

These are explicitly deferred beyond v1:

- Xwayland
- multiple overlay layers
- multiple displays
- generalized native app launcher/registry
- arbitrary overlay positioning rules
- provider UX for selecting many external app classes
- app-specific readback semantics for hosted native surfaces
- richer native surface types beyond the terminal/CLI initial target
- cross-platform compositor ambitions outside Linux

## Risks

### Risk 1: Host-mode complexity swallows the current v1 slice

Mitigation:
- require `winit` bring-up first
- do not start with DRM/KMS as the first proof point

### Risk 2: Native overlay scope balloons into general desktop support

Mitigation:
- lock v1 to one Wayland-native terminal/CLI overlay target
- explicitly defer Xwayland and general app compatibility

### Risk 3: Compositor becomes a second authority

Mitigation:
- keep provider topology as SSOT
- constrain compositor responsibilities to display policy, input routing, and native surface hosting

## Remaining Implementation Questions

These do not block the v1 spec and do not represent missing compositor-seam requirements, but they still need concrete implementation answers:

1. What is the exact provider/compositor wire shape (including versioning/compat policy) for the first `external/native surface` representation and runtime-bridge status?
2. For first host KMS bring-up, what is the smallest deterministic policy for connector/CRTC/device selection and recovery on hotplug or device-loss events?
3. What is the smallest concrete control-surface implementation for bootstrap/recovery operations (for example Unix socket, localhost RPC, or tiny CLI wrapper over the same local API)?

## Implementation Handoff

Engineers can start from this spec if they preserve these boundaries:

- build a separate Rust compositor project on Smithay
- prove the local `winit` path first
- keep `winit` development runtime and host DRM/KMS runtime as explicit, separate execution paths
- keep normal Surf Ace Linux app mode unchanged
- keep provider/protocol/topology ownership where it already belongs
- treat the terminal overlay as the first real external/native surface target, not as a placeholder `html` path

If implementation pressure pushes toward Electron-as-compositor, fake-HTML native hosting, or broad Xwayland/app-compat scope, that is a spec violation rather than a reasonable shortcut.
