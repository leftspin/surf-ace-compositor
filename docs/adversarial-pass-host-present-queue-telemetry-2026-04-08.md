# Surf Ace Compositor — Adversarial Review (Host Present-Queue Telemetry Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: runtime status truth for actually queued present behavior in `src/model.rs`, `src/state.rs`, and `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Runtime truth for “capable” vs “actually queued”

Result: **improved**.

Why:
- Runtime status now reports the last queued present source (`none`/`dumb`/`direct_gbm`) instead of only capability state.
- Runtime status now reports whether the most recent successful queue used atomic commit and whether an overlay plane was actually queued in that commit.
- Queue telemetry is written only after successful page-flip/atomic queue submission, so status reflects real queued behavior rather than optimistic intent.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Queue telemetry is cleared during start/preflight/failure/stopped transitions and when present ownership drops to `none`.
- Existing reclaim/fail-closed behavior is unchanged.
- Overlay-plane queue telemetry is derived from the same rotation/overlay-layout gating logic already used by atomic commit path.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This slice only improves host runtime observability; it does not change pane/overlay policy or topology authority boundaries.
- It strengthens control-path honesty for host runtime behavior already within compositor authority.

## Nits / Next-Slice Cautions

1. Telemetry reports last queued behavior, not full frame-history/cadence statistics.
2. Hardware validation remains required for final truth across real DRM drivers.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test runtime_host_present_queue_status_is_explicit_and_fail_closed` passes.
- `cargo test` passes.
- `RuntimeStatus` now includes last-queued present telemetry fields.
- Host queue path updates last-queued telemetry only on successful present queue.
- Fail-closed transitions and ownership-loss paths clear queued telemetry.

## Review Call

This slice closes a concrete honesty gap between “present capability exists” and “what the compositor actually queued most recently,” improving runtime/control truth without widening claims.
