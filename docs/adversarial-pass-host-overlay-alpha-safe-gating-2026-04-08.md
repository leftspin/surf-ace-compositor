# Surf Ace Compositor — Adversarial Review (Overlay Alpha-Safe Plane Gating Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: fail-closed overlay-plane eligibility in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay/direct-present truth when overlay plane lacks alpha-capable scanout format

Result: **improved**.

Why:
- Runtime now treats overlay plane as capable only when atomic overlay scanout format is alpha-capable.
- Per-frame overlay-plane split is now gated on both rotation support and alpha-capable overlay scanout format.
- Overlay GBM scanout state initialization now receives only alpha-capable overlay formats, avoiding accidental opaque overlay-plane composition.

### 2) Fail-closed runtime integrity

Result: **improved**.

Why:
- Drivers exposing only opaque overlay scanout formats no longer trigger optimistic overlay-plane usage.
- In that case, compositor remains on truthful single-plane composition paths instead of risking incorrect overlay blending.
- Existing host claim/reclaim and failure handling remain unchanged.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This slice tightens host present behavior for existing v1 overlay policy and does not alter pane authority, role admission, or topology ownership.
- It reduces capability overstatement and keeps runtime behavior explicit.

## Nits / Next-Slice Cautions

1. Alpha-safe format gating improves eligibility truth but does not fully prove cross-driver blend property behavior in hardware.
2. Final hardware proof still requires racter runs with direct atomic/overlay evidence capture.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test overlay_scanout_` passes.
- `cargo test` passes.
- `claimed_present_capabilities` now reports overlay-plane capability only for alpha-capable overlay scanout formats.
- `queue_claimed_presentation_tick` now gates overlay split usage on alpha-safe overlay formats.
- `claim_output_on_device` now passes only alpha-capable overlay scanout formats into renderer overlay scanout provisioning.

## Review Call

This slice closes a concrete fail-closed truth gap: overlay-plane composition is no longer treated as available when the selected overlay scanout format cannot preserve overlay alpha semantics.
