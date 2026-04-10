# Surf Ace Compositor — Adversarial Review (Overlay Blend-Control Gating Slice, 2026-04-10)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: atomic plane blend/alpha control truth for overlay-plane eligibility in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay-plane alpha truth vs implicit blend-property assumptions

Result: **improved**.

Why:
- Atomic plane property discovery now captures optional `alpha` and `pixel blend mode` controls.
- Overlay planes now only remain eligible when alpha-capable blend-mode enums are available (`premultiplied` or `coverage`) if the blend property exists.
- Overlay capability/status gating now requires both alpha-capable scanout format and alpha-capable blending support.

### 2) Atomic request truth and fail-closed behavior

Result: **improved**.

Why:
- Atomic requests now program discovered `alpha` and `pixel blend mode` values when available, making plane behavior explicit rather than driver-default implicit.
- If overlay blend-mode property exists but lacks alpha-capable modes, overlay routing is disabled for that output with explicit log evidence.
- Existing startup/recovery lifecycle and primary-path fallback behavior are unchanged.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This slice tightens runtime/hardware-facing present behavior only; pane authority, overlay admission policy, and topology boundaries are unchanged.
- It strengthens v1 overlay honesty by refusing optimistic overlay-plane claims on incompatible plane controls.

## Nits / Remaining Cautions

1. Drivers without explicit blend-mode property still rely on kernel/driver defaults; real hardware proof remains required.
2. This slice does not replace required on-device proof artifacts (`status` + logs + plane evidence + visual captures).

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test` passes.
- `AtomicPlaneState` now carries explicit composition-control decisions (`alpha`, `pixel_blend_mode`, `supports_alpha_blending`).
- `build_atomic_claim_plan` now configures composition controls and fail-closes overlay routing when blend controls cannot represent alpha-safe composition.
- `claimed_present_capabilities` and overlay split gating now require alpha-capable format **and** alpha-capable blend support.

## Review Call

This slice closes a concrete runtime/hardware-truth gap that blocked trustworthy overlay-plane claims: overlay routing is now contingent on explicit plane composition controls rather than optimistic assumptions.
