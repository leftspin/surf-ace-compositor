# Surf Ace Compositor — Adversarial Review (Hotplug/Rebind Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host hotplug/device-loss reclaim in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Is there real in-process reclaim/rebind instead of unconditional stop?

Result: **yes**.

Why:
- On udev-driven claimed-output loss, runtime now attempts in-process output reclaim (`claim_output_ownership(...)`) before failing the runtime.
- On successful reclaim, runtime rebinds the DRM event source to the newly claimed device FD and continues present loop operation.
- Runtime role geometry is reconfigured from the reclaimed mode and runtime dimensions are updated without pretending a restart happened.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- If no opened DRM device remains, runtime still fails closed.
- If reclaim cannot re-establish ownership, runtime fails closed with explicit failure reason.
- If reclaim succeeds but DRM event source cannot be rebound, runtime also fails closed.

### 3) No silent direct→dumb ownership downgrade during reclaim

Result: **enforced for direct-owned sessions**.

Why:
- Reclaim now carries a required ownership guard for sessions that originally established direct-present ownership.
- Claim path rejects reclaim when direct startup modeset cannot be re-established under that requirement.

### 4) Spec alignment / authority boundaries

Result: **aligned**.

Why:
- No topology-authority drift.
- Host-vs-winit runtime split remains explicit.
- This is backend lifecycle handling only; pane/overlay authority boundaries remain unchanged.

## Nits / Next-Slice Cautions

1. Reclaim is still opportunistic around udev add/change/remove events; broader DRM failure classes (e.g. commit-time EIO sequences without udev transition) still terminate runtime.
2. Output reclaim currently rebinds a single claimed output path; multi-output plane/state migration logic is still out of scope.
3. No dedicated automated integration harness yet for synthetic udev/device-loss reclaim behavior; hardware/system-level validation is still required.

## Evidence Snapshot

- `cargo fmt` and `cargo test` pass.
- Reclaim entry point in udev handling now attempts claim instead of immediate stop.
- DRM event source registration is now explicit helper logic and is rebound on reclaim.
- Direct-owned reclaim guard blocks silent downgrade to non-direct startup ownership.

## Review Call

This is honest progress toward drm-compositor-grade device-loss handling: reclaim/rebind now exists in-process with fail-closed semantics and direct-ownership truth preserved.
