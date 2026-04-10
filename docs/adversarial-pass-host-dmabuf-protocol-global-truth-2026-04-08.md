# Surf Ace Compositor — Adversarial Review (Host DMABUF Protocol Global Truth, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host dmabuf protocol/global lifecycle truth in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) DMABUF protocol truth vs actual host renderer capability

Result: **improved**.

Why:
- Host startup now syncs dmabuf protocol formats from the claimed output pipeline directly, including the `None` case when no claimed GLES/dmabuf renderer exists.
- Runtime dmabuf sync now supports disabling the dmabuf global completely when no truthful format set exists, instead of keeping a stale static/global claim.
- Reclaim and device-loss windows now fail closed for dmabuf protocol truth by clearing advertisement before reclaim and re-enabling only after a successful claim with real formats.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- `running` semantics remain tied to real output claim/ownership; this slice only tightened protocol advertisement behavior.
- Reclaim logic still follows existing control/recovery flow, with no new optimistic runtime state transitions.
- Reclaimable present/event failures now explicitly drop dmabuf protocol advertisement while output ownership is absent.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This change does not alter pane topology, role policy, or provider/compositor authority boundaries.
- The slice removes a capability over-claim at the Wayland protocol boundary, which is consistent with fail-closed host-runtime guidance in the adversarial spec review.

## Nits / Next-Slice Cautions

1. DMABUF protocol truth now tracks whether formats exist, but does not yet expose richer runtime status details (for example, active dmabuf format/modifier set).
2. Hardware validation is still required to prove broader multi-format/multi-plane behavior on real drivers.

## Evidence Snapshot

- `cargo test overlay_plane_layout_maps_overlay_rect_to_atomic_coordinates` passes.
- `cargo test` passes.
- `RuntimeWaylandState::sync_dmabuf_protocol_formats` now accepts optional format sets and disables/recreates the dmabuf global accordingly.
- Host startup and reclaim paths now call that sync with claimed renderer formats (or `None`), and claim-loss paths clear protocol advertisement immediately.

## Review Call

This is honest progress: dmabuf protocol advertisement now matches real claimed renderer capability across startup, loss, and reclaim, instead of leaking stale capability claims.
