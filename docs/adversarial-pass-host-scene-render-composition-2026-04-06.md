# Surf Ace Compositor — Adversarial Review (Host Scene/Render Composition Substrate, 2026-04-06)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- `docs/adversarial-pass-host-presentation-substrate-2026-04-06.md`
- implementation focus: host composition/presentation updates in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress vs fake/theatrical

Result: **real progress**.

Why:
- Host path now composes scene output from committed Wayland `wl_shm` surface buffers (main, overlay, popup roots) into the scanout back buffer before each page flip.
- This replaces purely synthetic frame coloring as the primary frame source.
- Page-flip completion now drives frame-callback delivery/flush for active surfaces, creating a real compositor frame loop.

### 2) `running` honesty

Result: **still honest**.

Why:
- Runtime still transitions to `running` only after successful DRM/KMS output claim.
- Composition/presentation errors still fail closed.

### 3) Control/recovery truth

Result: **preserved**.

Why:
- Existing host failure survivability test continues to pass (`tests/host_failure_survivability.rs`).
- Retry/start control path behavior is unchanged.

### 4) Spec alignment / authority boundaries

Result: **aligned**.

Why:
- No second topology authority introduced; pane/runtime role policy remains unchanged.
- Prototype overlay policy remains separate from long-term pane-hosting contract.
- Dev `winit` path vs host DRM path split remains explicit.

## Nits / Next-Slice Cautions

1. Current host scene composition is `wl_shm`-only and root-surface oriented; dmabuf/EGL clients and full subsurface trees are not yet in this path.
2. This is still a software composition bridge into dumb buffers; GBM/EGL renderer-backed scene composition remains the major next milestone.
3. Hardware validation is still required for confidence across real DRM driver/output combinations.

## Evidence Snapshot

- `cargo fmt && cargo test` passes.
- Host runtime now executes: scene compose -> DRM page flip -> page-flip event -> frame callbacks.

## Review Call

This slice is an honest advance from presentation substrate toward real compositor scene/render behavior.
It does not introduce new product-truth failures in reviewed scope.
