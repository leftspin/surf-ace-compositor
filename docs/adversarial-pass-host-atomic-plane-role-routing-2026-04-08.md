# Surf Ace Compositor — Adversarial Review (Atomic Plane-Role Routing Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: atomic commit plane routing in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Plane routing truth vs implicit plane ordering

Result: **improved**.

Why:
- Atomic plane state now carries explicit role metadata (`primary` vs `overlay`) at claim-plan construction time.
- Atomic startup-claim and per-frame commit paths now route framebuffer/layout assignment by explicit plane role instead of hard-coded vector index assumptions.
- Overlay plane disable/enable behavior remains explicit and is now tied to role semantics rather than list position.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Runtime running/reclaim behavior is unchanged.
- Atomic commit failure handling remains unchanged and still uses existing reclaimable failure classification paths.
- This slice changes only mapping logic inside existing atomic request construction.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- No topology or pane-authority boundaries changed.
- Overlay role admission and overlay geometry policy are unchanged.
- The slice tightens host present-path implementation robustness without widening capability claims.

## Nits / Next-Slice Cautions

1. Full multi-plane orchestration policy (beyond current primary+overlay roles) is still future work.
2. Hardware validation is still required across drivers for atomic role routing behavior.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test overlay_plane_layout_maps_overlay_rect_to_atomic_coordinates` passes.
- `cargo test` passes.
- `AtomicPlaneState` now includes explicit `AtomicPlaneRole`.
- `claim_output_with_atomic_modeset` and `queue_atomic_frame_commit` now map framebuffers/layouts via role-based matching.

## Review Call

This slice closes a concrete robustness gap by replacing index-dependent atomic plane routing with explicit role-based routing, keeping the current product truth while reducing brittle present-path assumptions.
