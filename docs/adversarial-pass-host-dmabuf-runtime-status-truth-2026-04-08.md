# Surf Ace Compositor — Adversarial Review (Host DMABUF Runtime-Status Truth Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: runtime/control observability for dmabuf protocol advertisement truth in `src/model.rs`, `src/state.rs`, and `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Runtime observability of dmabuf protocol truth

Result: **improved**.

Why:
- Runtime status now explicitly reports whether dmabuf protocol advertisement is enabled and which format/modifier pairs are currently advertised.
- Wayland runtime initialization and dmabuf format sync now push current protocol truth into shared runtime status, so control/status output matches actual advertised protocol state.
- Recovery/failure transitions now clear dmabuf runtime status fields fail-closed, preventing stale capability claims in status snapshots.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Running semantics and output-ownership gates are unchanged.
- Existing dmabuf global lifecycle behavior is unchanged; this slice adds status truth plumbing and fail-closed clearing behavior in runtime state transitions.
- Host startup/reclaim/claim-loss code paths continue to sync dmabuf protocol formats through one runtime path.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- The slice only tightens control/runtime observability and does not alter pane authority, role policy, or output ownership authority.
- This matches spec guidance that host runtime behavior should remain explicit and verifiable rather than implicit.

## Nits / Next-Slice Cautions

1. Status now surfaces active dmabuf protocol set, but still does not include deeper per-plane hardware commit telemetry.
2. Real hardware validation remains required for broader multi-format/multi-plane behavior.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test runtime_dmabuf_protocol_status_is_explicit_and_fail_closed` passes.
- `cargo test` passes.
- `RuntimeStatus` now contains `dmabuf_protocol_enabled` and `dmabuf_protocol_formats`.
- `CompositorState` now has explicit setter/reset behavior for dmabuf protocol status across start/preflight/failure/stopped transitions.
- `RuntimeWaylandState::sync_dmabuf_protocol_formats` now always updates shared runtime status to match the currently advertised protocol set.

## Review Call

This slice closes a concrete honesty gap in runtime observability: control/status now exposes the real dmabuf protocol advertisement state and clears it fail-closed during failures and restart transitions.
