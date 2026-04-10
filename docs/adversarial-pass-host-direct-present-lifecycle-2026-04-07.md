# Surf Ace Compositor — Adversarial Review (Direct Present Lifecycle Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: direct-present lifecycle behavior in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real lifecycle/commit progress beyond legacy claim flow

Result: **yes**.

Why:
- Startup claim now records explicit startup present ownership mode (`direct_gbm` vs `dumb`).
- When startup ownership is `direct_gbm`, frame queue path now requires direct-present continuity and refuses silent downgrade to dumb/readback.
- Direct scanout state is now explicitly re-ensured/rebuilt as part of direct render path (`ensure_direct_scanout_state`) instead of being best-effort optional state.

### 2) Running/recovery truth and fail-closed semantics

Result: **preserved and tightened**.

Why:
- `running` still requires successful output ownership.
- Direct-owned sessions now fail closed if direct-present frame production cannot continue, instead of quietly degrading.
- Dumb fallback remains available for startup paths that were honestly dumb-owned.

### 3) Spec alignment / authority boundaries

Result: **aligned**.

Why:
- No second topology authority introduced.
- Dev/runtime mode split remains explicit.
- Prototype overlay policy and long-term pane-hosting contract remain separate.

## Nits / Next-Slice Cautions

1. Startup still uses legacy `set_crtc` for ownership transition; direct lifecycle is stronger after claim, but claim primitive itself is still legacy KMS.
2. This slice is still not full drm-compositor/atomic commit orchestration (plane state management, atomic transaction lifecycle, etc.).
3. Hardware validation remains necessary to confirm direct-owned fail-closed behavior across real devices.

## Evidence Snapshot

- `cargo fmt && cargo test` passes.
- Direct-owned startup now enforces direct-present continuity in queue path (`requires_direct_present`).
- Lazy dumb fallback allocation remains only for dumb-owned startup paths.

## Review Call

This slice is honest progress toward direct-present lifecycle/commit truth beyond legacy claim-only behavior.
No new real product-truth failures were introduced in reviewed scope.
