# Surf Ace Compositor — Adversarial Review (Overlay-Plane Rotation Gating Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: rotation-aware overlay-plane direct/atomic path behavior in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay-plane truth under rotated outputs

Result: **improved**.

Why:
- Overlay-plane composition is now explicitly gated to `deg0` output rotation.
- When output rotation is non-`deg0`, overlay-plane framebuffer generation is disabled and overlay content remains on the primary-plane composition path (which already applies the output transform).
- Atomic overlay-plane layout programming now also checks rotation before enabling overlay layout, preventing rotated-output overlay-plane claims the compositor cannot yet realize truthfully.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Runtime running/reclaim semantics are unchanged.
- Direct-present and fallback error paths are unchanged.
- The slice narrows behavior (fail-closed overlay-plane disable on unsupported rotation) instead of widening claims.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- Provider/compositor authority boundaries remain unchanged.
- Overlay role and pane lifecycle gating remain unchanged.
- This is a rendering/present honesty correction in host runtime policy, consistent with spec fail-closed guidance.

## Nits / Next-Slice Cautions

1. Overlay-plane rotation support itself is still not implemented; this slice is explicitly a truthful disable, not feature completion.
2. Hardware validation is still required for multi-plane behavior on real DRM drivers once rotation-capable overlay handling is implemented.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test runtime::tests::runtime_overlay_policy_rect_maps_directly_to_atomic_overlay_layout` passes.
- `cargo test` passes.
- `queue_claimed_presentation_tick` now gates overlay-plane split usage on current output rotation.
- `render_host_scene_with_gles_direct` now skips overlay-plane framebuffer generation when overlay-plane split is not truthful.
- `queue_atomic_frame_commit` now refuses overlay-plane layout activation on non-`deg0` rotation.

## Review Call

This slice closes a concrete honesty gap by preventing rotated-output overlay-plane behavior that the current renderer/atomic path does not truthfully support.
