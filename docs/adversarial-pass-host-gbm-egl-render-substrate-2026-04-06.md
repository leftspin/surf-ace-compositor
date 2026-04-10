# Surf Ace Compositor — Adversarial Review (Host GBM/EGL/GLES Render Substrate, 2026-04-06)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host render-path updates in `Cargo.toml` and `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Is this real progress toward host render path, not theater?

Result: **yes, real progress**.

Why:
- Host claim path now initializes a real GBM/EGL/GLES renderer substrate on the claimed DRM device.
- Host frame tick now prefers GLES scene rendering over software-only `wl_shm` blit path.
- Scene rendering path uses compositor scene elements (main/overlay/popup role geometry) and renderer draw path before page flip.

### 2) Running/recovery truth

Result: **preserved**.

Why:
- `running` transition remains gated by successful DRM output ownership claim.
- Host frame queue/event errors still fail runtime closed.
- Startup failure survivability and control retry path remain intact (`cargo test` includes host survivability integration test).

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- No second topology authority was introduced.
- Overlay/main role policy and pane-authoritative admission path are unchanged.
- Host and dev runtime paths remain distinct.

## Nits / Next-Slice Cautions

1. Current GLES path renders to an offscreen texture and readbacks into dumb scanout buffers; direct GBM/dmabuf-presented render path is still pending.
2. If GLES substrate initialization or render fails, runtime falls back to software `wl_shm` composition for continuity; this keeps behavior safe but is not the end-state render path.
3. Full client coverage still needs dmabuf/EGL client buffer handling and deeper subsurface treatment in host composition.

## Evidence Snapshot

- `smithay` now enables `backend_gbm` in this repo.
- Host pipeline now includes GBM/EGL/GLES renderer state and frame-path integration.
- `cargo fmt && cargo test` passes.

## Review Call

This slice is an honest substrate step from software composition toward a real host render path.
It does not introduce new product-truth failures in reviewed scope.
