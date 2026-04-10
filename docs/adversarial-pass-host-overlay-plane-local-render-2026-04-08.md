# Surf Ace Compositor — Adversarial Review (Overlay-Plane Local Render Truth, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host overlay-plane framebuffer render path in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay-plane framebuffer content truth

Result: **improved**.

Why:
- Overlay-plane rendering now builds a dedicated local-space overlay element list instead of reusing output-space overlay elements.
- The overlay-plane path now includes overlay-owned popups in local coordinates, so attached transient content follows the overlay plane path rather than being implicitly lost.
- The overlay plane is disabled when there is no overlay surface, invalid overlay geometry, or no overlay-local render content.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Runtime `running`/claim semantics are unchanged; this slice only changes per-frame render composition for overlay-plane targets.
- Any existing direct-render/present failure still follows the existing reclaim/fallback path.
- The overlay plane is only supplied a framebuffer when a real local render pass succeeds.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- Pane geometry/identity authority remains in runtime/provider state (`overlay_rect` and role ownership are unchanged).
- Overlay role admission and lifecycle gating remain subordinate to pane lifecycle truth.
- This change tightens how overlay-owned transient surfaces are realized in the host renderer, consistent with spec invariant that transient children stay attached to pane ownership.

## Nits / Next-Slice Cautions

1. Main-plane composition still renders the full scene list, so overlay content can still be present on primary composition while also being produced for overlay-plane composition; deeper plane-partitioned composition truth remains a follow-up slice.
2. Real hardware validation is still required to prove final plane blend/visibility behavior across drivers.

## Evidence Snapshot

- `cargo test runtime::tests::overlay_plane_layout_maps_overlay_rect_to_atomic_coordinates` passes.
- `cargo test` passes.
- `render_overlay_plane_framebuffer` now renders overlay-local content and returns `None` when no truthful overlay framebuffer should be queued.
- `collect_overlay_plane_elements_local` now imports overlay toplevel and overlay-owned popup trees in local coordinates.

## Review Call

This slice closes a real overlay-plane rendering honesty gap: the overlay framebuffer now carries local overlay content rather than output-space-positioned artifacts.
