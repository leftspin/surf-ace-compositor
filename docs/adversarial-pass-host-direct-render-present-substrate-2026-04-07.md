# Surf Ace Compositor — Adversarial Review (Direct Render/Present Substrate, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host direct-present substrate in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress vs fake/theatrical

Result: **real progress**.

Why:
- Host path now allocates GBM scanout-capable buffers and exports DRM framebuffers from those buffers.
- GLES host composition can now render directly into dmabuf-backed GBM scanout buffers.
- Page flips now queue those GBM-exported framebuffers when direct path succeeds, instead of always flipping readback-populated dumb buffers.

### 2) Running semantics and fail-closed behavior

Result: **preserved**.

Why:
- `running` transition remains gated by successful DRM/KMS output ownership claim.
- Flip queue and DRM event-processing failures still fail runtime closed.
- If direct path initialization/render fails, host path degrades to previous readback/software path without faking direct-present success.

### 3) Recovery/control truth

Result: **preserved**.

Why:
- Host startup failure survivability test still passes (`tests/host_failure_survivability.rs`).
- Control retry/start behavior is unchanged.

### 4) Spec alignment

Result: **aligned**.

Why:
- No second topology authority introduced.
- Prototype overlay policy and long-term pane-hosting contract remain separate.
- Host runtime path remains distinct from `winit` development path.

## Nits / Next-Slice Cautions

1. Initial modeset is still performed with dumb fb claim; direct GBM path takes over during subsequent flip queue.
2. Direct path still uses simplified two-buffer policy and does not yet provide full KMS atomic/drm-compositor management.
3. dmabuf/EGL client coverage and deeper subsurface composition semantics are still not complete for full product-level host rendering.

## Evidence Snapshot

- `cargo fmt && cargo test` passes.
- Host queue path now tracks flip source (`dumb` vs `direct_gbm`) and swaps the corresponding buffer ring on pageflip completion.

## Review Call

This slice is an honest reduction of the readback bridge toward direct render/present behavior.
No new real product-truth failures were introduced in reviewed scope.
