# Surf Ace Compositor Impl Pass 6 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Take the first honest step into actual host compositor mode.

## Slice choice

This pass prioritized a non-fake host backend runtime slice over additional side mechanics:

- added an executable `host` runtime path with real `libseat` session + `udev` DRM scanning + seat-mediated DRM device opening
- explicitly kept `winit` and `host` runtime paths separate

## What was implemented

1. New runtime backend mode:
   - `RuntimeBackend::HostDrm`
   - CLI runtime selector now accepts `--runtime host`
2. Real host backend preflight/runtime structure:
   - create `libseat` session (`LibSeatSession::new`)
   - derive seat name from session
   - monitor seat devices via `UdevBackend`
   - enumerate initial DRM devices from `udev`
   - attempt to open each DRM card through session-mediated `session.open(...)`
   - keep opened device fds alive while runtime is active; close via session on drop/remove
   - fail startup if no DRM devices are detected or if none can be opened
3. Runtime status host snapshot:
   - added `host_seat_name`
   - added `host_detected_drm_device_count`
   - added `host_opened_drm_device_count`
   - added `host_primary_drm_path`
4. Runtime event handling for host slice:
   - session pause event marks runtime failed and stops loop
   - `udev` add/change/remove updates host-device snapshot and open-device set

## Adversarial findings during implementation

1. Risk: introducing a nominal “host mode” that does not actually attempt seat-managed DRM ownership would be fake integration.

Fix:

1. required real `session.open` attempts against discovered `/dev/dri/card*` devices; startup is considered failed if none can be opened.

2. Risk: collapsing `winit` and host behavior into one generic path could blur product/runtime truth.

Fix:

2. preserved explicit runtime mode split (`none` / `winit` / `host`) with host-only backend code path.

## Spec-check verdict for this pass

Newly satisfied in this pass:

1. Real host backend path exists as executable runtime structure (`libseat` + `udev` + DRM open attempts), not just abstract architecture.
2. Backend distinction is explicit and observable (`winit` dev path vs `host` compositor path).
3. Compositor now performs real seat-mediated device acquisition attempts toward output/input ownership.

Preserved constraints:

1. Provider topology authority remains outside compositor runtime.
2. Prototype overlay policy remains separate from long-term pane-hosting contract.
3. No fake `html` modeling for native surfaces.
4. Control path remains tiny; no second authority path introduced.

Not started in this pass:

1. Full DRM/KMS rendering + connector/crtc mode-set + frame loop bring-up.
2. libinput device integration and host input event routing.

## Validation run

Executed after implementation:

- `cargo check` (pass)
- `cargo test` (pass, 10/10)

Host runtime smoke in this environment:

- `cargo run -- serve --runtime host ...`
- observed real behavior: detected `/dev/dri/card*`, attempted session-mediated opens, failed with `Operation not permitted`, then exited with `HostNoDrmDeviceOpened`.
- this is expected under restricted seat permissions and confirms non-fake backend preflight execution.
