# Surf Ace Compositor — Adversarial Review (Typed Recovery Classification Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host recovery classification in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Did recovery decisions move off string-pattern matching?

Result: **yes**.

Why:
- Recovery classification is now explicit in typed runtime structures (`HostPresentFailureClass`, `HostPresentFailure`).
- Present queue and event processing return typed failure results, with recoverability chosen at error creation points rather than parsed from text.
- Recovery decision sites (`timer` queue path and DRM event source path) now branch on `failure.is_reclaimable()`.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- Reclaim still requires successful output ownership + event-source rebind before runtime continues.
- Non-reclaimable failures still mark runtime failed and stop.
- No silent running-state overclaim was introduced.

### 3) Recovery scope discipline

Result: **maintained**.

Why:
- Reclaimable classification remains limited to known commit/present paths (queue page-flip / queue atomic commit / DRM event read stream failures).
- Other failures remain fatal by default via `From<RuntimeError> for HostPresentFailure`.

### 4) Ownership-mode honesty and spec alignment

Result: **preserved**.

Why:
- Direct-owned reclaim guard remains enforced; no direct->dumb silent downgrade.
- No authority-boundary drift or runtime-mode conflation was introduced.

## Nits / Next-Slice Cautions

1. Recovery classification is still local to runtime glue; it is not yet promoted to a broader backend error taxonomy shared across modules.
2. Failure reason observability is still mostly string payloads inside `RuntimeError`, even though recovery decisions are typed.
3. Hardware integration tests that intentionally trigger recoverable vs fatal present failures are still missing.

## Evidence Snapshot

- `cargo fmt` and `cargo test` pass.
- Typed classes introduced: `HostPresentFailureClass`, `HostPresentFailure`.
- `queue_claimed_presentation_tick` and `process_claimed_presentation_events` now return typed failure results.
- String-pattern classifier function was removed; recovery decisions now read typed class.

## Review Call

This is honest progress: recovery policy no longer depends on brittle string matching and now runs on explicit typed failure classification, while preserving fail-closed and ownership-truth constraints.
