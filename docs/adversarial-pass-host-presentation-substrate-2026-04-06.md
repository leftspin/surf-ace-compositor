# Surf Ace Compositor — Adversarial Review (Host Presentation Substrate Slice, 2026-04-06)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host presentation path updates in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress vs fake/theatrical

Result: **real progress**.

Why:
- Host path now maintains a presentation pipeline with two compositor-owned DRM dumb buffers.
- After ownership claim (`set_crtc`), runtime now performs recurring `page_flip` submissions and handles DRM page-flip completion events.
- This is not status-only simulation; it is real KMS presentation activity on claimed output.

### 2) Does `running` remain honest?

Result: **yes**.

Why:
- Host runtime still enters `running` only after successful output ownership claim.
- Presentation failures (queueing flips or processing DRM events) now fail the runtime explicitly instead of silently degrading.

### 3) Control/recovery truth

Result: **preserved**.

Why:
- Existing failure survivability behavior remains intact (`cargo test` includes `tests/host_failure_survivability.rs`).
- Startup/retry control path is unchanged by this slice.
- Host path remains fail-closed if claimed output is lost.

### 4) Spec alignment and drift risk

Result: **aligned for this milestone**.

Why:
- `winit` dev path and host DRM path remain separate.
- This slice advances real host presentation substrate without overclaiming full compositor rendering completion.
- No second topology authority is introduced; pane/runtime authority boundaries are unchanged.

## Nits / Next-Slice Cautions

1. Rendering is still a minimal host-owned frame source (animated dumb-buffer colors), not yet GBM-backed scene composition of Wayland surfaces.
2. Real hardware validation remains required for confidence across varied DRM drivers/connectors.
3. Claimed-output observability (connector/crtc ids, flip cadence stats) is still minimal.

## Evidence Snapshot

- `cargo fmt && cargo test` passes.
- Runtime code now includes explicit page-flip scheduling + page-flip event handling on the claimed DRM device.

## Review Call

This slice is an honest substrate step toward a real host render/presentation pipeline.
It does not appear theatrical, and it does not introduce new product-truth failures in reviewed scope.
