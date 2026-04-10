# Surf Ace Compositor — Adversarial Review (Commit/Present Failure Recovery Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host commit/present failure recovery in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Recovery beyond udev-driven loss

Result: **yes**.

Why:
- Timer path now attempts in-process reclaim when claimed output is missing, independent of a udev event.
- Queue-path commit/present failures in selected classes now trigger reclaim attempts instead of immediate runtime stop.
- DRM event-stream failures (`receive_events`) now demote claimed output and schedule reclaim instead of immediate terminal failure.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Recovery only counts when output claim + event-source rebind succeeds.
- If reclaim or event-source rebind fails, runtime is explicitly marked failed and stopped.
- No silent running-state overclaim was introduced.

### 3) Ownership-mode honesty

Result: **preserved**.

Why:
- Reclaim continues to honor startup direct-present ownership requirement.
- Direct-owned sessions do not silently downgrade to dumb startup ownership during recovery.

### 4) Spec/authority alignment

Result: **aligned**.

Why:
- No second topology authority introduced.
- Host/runtime split remains explicit (`winit` vs host DRM).
- This is host lifecycle behavior only; pane/overlay authority boundaries are unchanged.

## Nits / Next-Slice Cautions

1. Recovery classification is currently string-pattern based for commit/present failure classes; richer typed error classification would be more robust.
2. Recovery still targets single claimed output semantics; no multi-output policy/migration yet.
3. No hardware integration harness yet to force commit/present error classes deterministically and validate reclaim paths across driver variants.

## Evidence Snapshot

- `cargo fmt` and `cargo test` pass.
- New failure classifier: `should_attempt_present_reclaim(...)`.
- Shared reclaim helper used across timer/udev paths: `reclaim_host_output_in_process(...)`.
- DRM event source now marks recoverable present/event failures for in-process reclaim: `bind_claimed_drm_event_source(...)`.

## Review Call

This slice is honest progress toward drm-compositor-grade recovery: host runtime can now recover in-process from selected commit/present failure classes beyond udev-driven output loss, while remaining fail-closed when reclaim cannot be proven.
