# Surf Ace Compositor — Adversarial Review (Host Queued Framebuffer Format Telemetry Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: queue-time host present telemetry for actual queued framebuffer format/modifier truth in `src/model.rs`, `src/state.rs`, and `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Runtime truth for broader format/modifier behavior on queued presents

Result: **improved**.

Why:
- Runtime status now records the last queued primary-plane dmabuf format/modifier and the last queued overlay-plane dmabuf format/modifier (when known).
- Telemetry is derived from the same direct/overlay GBM buffers actually queued for present, so status reflects queued path truth instead of static capability assumptions.
- Non-dmabuf queue paths (dumb fallback) intentionally report format as unknown (`None`) rather than fabricating dmabuf metadata.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- New format telemetry fields are cleared on startup/preflight/failure/stopped transitions and on ownership drop to `none`.
- Queue telemetry continues to update only after successful present queue submission.
- Existing host ownership/reclaim semantics are unchanged.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This slice improves runtime observability of host present behavior; it does not widen compositor authority over pane identity/topology.
- It supports the spec’s explicit/runtime-verifiable behavior requirement without claiming hardware proof.

## Nits / Next-Slice Cautions

1. This is still last-queued telemetry, not longitudinal per-frame history/cadence.
2. Real hardware validation is still required for full multi-driver multi-plane truth.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test runtime_host_present_queue_status_is_explicit_and_fail_closed` passes.
- `cargo test` passes.
- `RuntimeStatus` now includes:
  - `host_last_queued_primary_dmabuf_format`
  - `host_last_queued_overlay_dmabuf_format`
- Host queue path now sets those fields from actual queued GBM buffers when present source is direct/overlay dmabuf-backed.
- Fail-closed transitions clear the fields explicitly.

## Review Call

This slice closes a concrete observability gap after queue-source telemetry: status can now show what dmabuf format/modifier was actually queued on primary/overlay paths (when known), which raises honesty for format/multi-plane runtime truth without overstating hardware proof.
