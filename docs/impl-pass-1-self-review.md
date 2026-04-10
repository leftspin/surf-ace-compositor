# Surf Ace Compositor Impl Pass 1 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Implemented slice

Vertical slice delivered in this pass:

- compositor core state + policy in Rust
- tiny Unix socket control path used for both bootstrap/recovery-style queries (`get_status`, rotation) and runtime pane mode operations
- provider-driven pane snapshot ingestion (`apply_provider_snapshot`) as topology/geometry authority input
- live pane mode switching:
  - `surf_ace_rendered`
  - `external_native` with terminal-first exec/process payload
- external/native lifecycle state machine:
  - `absent`
  - `launching`
  - `attached`
  - `failed`
  - `exited`
- explicit reduced/adapted external-native event contract exposure
- prototype overlay policy enforcement (one active overlay pane at a time) isolated from the long-term pane-mode model

## Adversarial cycle

Cycle 1 findings:

1. `switch_pane_to_external_native` reserved overlay slot before pane existence validation.
2. This could leak prototype-policy state on bad pane IDs.

Fixes made:

1. reordered validation to check pane existence and running state before reserving overlay slot
2. added regression test: `missing_pane_does_not_reserve_overlay_slot`

Exit condition for this pass:

- no known invariant violation inside the shipped slice
- `cargo test` clean with invariant-focused tests

## Invariant status (slice-local)

Satisfied in this pass:

1. topology authority stays with provider for pane geometry (`apply_provider_snapshot` only)
2. pane mode switching is live and reversible
3. external/native is explicit content kind, not `html`
4. external/native payload is exec/process spec under Surf Ace authority
5. tiny control path handles bootstrap/runtime operations through one surface
6. prototype overlay policy is explicit and separate from long-term pane contract
7. reduced/adapted event contract is explicit for external/native mode

Partially addressed:

1. host-mode detection path exists (`host_mode_active` in control status), but Surf Ace client wiring is not integrated in this repo yet
2. terminal target lifecycle is implemented at process level; Wayland surface attachment is represented by explicit state transitions but not Smithay-surface-bound yet

Not yet implemented (next passes):

1. Smithay compositor runtime bring-up (`winit`, then DRM/KMS host mode)
2. fullscreen Surf Ace app role management and deterministic z-layer composition with real Wayland surfaces
3. input/focus routing between fullscreen app and overlay app in compositor runtime
4. output rotation execution in compositor backend (control state exists, runtime application pending)
5. transient child surface policy enforcement in real surface graph
