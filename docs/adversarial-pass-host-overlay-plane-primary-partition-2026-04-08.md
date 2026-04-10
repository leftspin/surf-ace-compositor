# Surf Ace Compositor — Adversarial Review (Overlay-Plane Primary Partition Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: direct-present primary/overlay composition partitioning in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay-plane commit truth vs primary-plane render content

Result: **improved**.

Why:
- Direct render now gates a plane-partitioned primary composition mode on real atomic overlay-plane availability.
- When an overlay framebuffer is actually produced, the primary plane draw list excludes overlay-owned content, avoiding duplicated overlay rendering across primary + overlay planes.
- When overlay-plane composition is not active (or no overlay framebuffer is produced), the renderer keeps full-scene primary composition so single-plane behavior is preserved.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Runtime running and reclaim semantics are unchanged.
- Direct-present failure handling and fallback behavior are unchanged.
- Overlay plane remains enabled only when a real overlay framebuffer exists.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- Pane/overlay authority still comes from runtime policy (`overlay_rect`, role ownership, and PID-gated overlay admission).
- This slice only changes how already-authorized scene elements are partitioned across planes.
- Overlay-owned transient surfaces remain attached to overlay content paths, aligning with pane ownership invariants.

## Nits / Next-Slice Cautions

1. This slice still relies on simplified plane ordering assumptions and does not introduce full plane transaction policy orchestration.
2. Hardware validation is still required to confirm expected blend/visibility behavior across real DRM drivers.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test runtime::tests::runtime_overlay_policy_rect_maps_directly_to_atomic_overlay_layout` passes.
- `cargo test` passes.
- `queue_claimed_presentation_tick` now propagates overlay-plane availability to direct render.
- `render_host_scene_with_gles_direct` now chooses primary draw list based on overlay framebuffer truth.
- `collect_render_elements` now tracks main/main-popup vs overlay/overlay-popup groups for deterministic primary-plane partitioning.

## Review Call

This slice closes a real compositor truth gap by preventing primary+overlay double-painting when overlay-plane presentation is active, while preserving fallback behavior for non-overlay-plane paths.
