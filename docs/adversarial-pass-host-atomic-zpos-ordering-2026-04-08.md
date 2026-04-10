# Surf Ace Compositor — Adversarial Review (Atomic Z-Order Programming Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: atomic primary/overlay plane ordering determinism in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay-above-main truth vs driver-default plane ordering

Result: **improved**.

Why:
- Atomic plane property discovery now captures optional `zpos` range support per plane.
- Claim-plan construction now assigns explicit primary/overlay `zpos` values when both planes expose compatible mutable+atomic `zpos` properties.
- Atomic startup modeset and per-frame atomic commits now program those `zpos` values, replacing implicit driver-default ordering for compatible devices.

### 2) Fail-closed behavior on unsupported/incompatible z-order controls

Result: **preserved**.

Why:
- When `zpos` is absent, non-atomic, immutable, or ranges cannot satisfy `primary < overlay`, the compositor does not force invalid property writes.
- In that case, plane assignment still proceeds via existing role routing and logs an explicit warning about falling back to driver defaults.
- Existing output claim/recovery behavior is unchanged.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- This slice tightens host present-path behavior for the existing one-main + one-overlay policy and does not alter pane authority or overlay admission policy.
- It improves determinism for the v1 fixed layering expectation without widening compositor product scope.

## Nits / Next-Slice Cautions

1. Explicit `zpos` programming is opportunistic and depends on per-driver property support.
2. Blend/alpha semantics across planes still require hardware validation on real targets.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test atomic_plane_zpos_selection` passes.
- `cargo test` passes.
- `AtomicPlanePropertyHandles` now carries optional `zpos` metadata.
- `build_atomic_claim_plan` now derives and stores deterministic `zpos` values for primary/overlay planes when representable.
- `populate_atomic_plane_properties` now writes `zpos` properties into atomic requests when configured.

## Review Call

This slice closes a concrete runtime/hardware-facing gap left after role-based plane routing: for drivers with usable `zpos`, overlay-above-main ordering is now explicitly programmed instead of assumed.
