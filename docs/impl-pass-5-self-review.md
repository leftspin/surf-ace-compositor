# Surf Ace Compositor Impl Pass 5 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Take the next honest step toward real host/compositor mode and stronger runtime/product bridging.

## Slice choice

This pass intentionally prioritized runtime/product bridge tightening over DRM/KMS backend scaffolding.

Rationale:

- In the current headless environment, adding DRM/KMS code paths without session/device integration and real exercise would be speculative plumbing.
- The tighter runtime/product bridge is a concrete, testable advancement of v1 provider-facing seam truth.

## What was implemented

1. Runtime overlay admission now requires bridge truth from pane state:
   - runtime only binds an `overlay_native` toplevel when state reports an active overlay pane with `external/native` render mode and lifecycle in `launching|attached`
   - otherwise runtime denies/closes the candidate toplevel and increments denied counter
2. Runtime role lifecycle now updates pane lifecycle for active overlay pane:
   - overlay role attach transitions active overlay pane `launching -> attached`
   - overlay role detach transitions active overlay pane `attached -> launching`
3. Runtime status now explicitly carries bridge association:
   - `runtime.overlay_bound_pane_id` is populated when an overlay role surface is bound
4. Policy visibility helper added:
   - prototype policy now exposes read-only `active_overlay_pane` accessor used by bridge logic

## Adversarial findings during implementation

1. A naive bridge could have allowed overlay role binding when no pane-level overlay request existed, reintroducing implicit peer authority.

Fix:

1. Added runtime-side admission check against state-level `runtime_overlay_binding_expected` truth before binding overlay role.

## Spec-check verdict for this pass

Newly satisfied in this pass:

1. Provider-facing bridge seam is more explicit: runtime role/surface lifecycle now materially updates pane external/native lifecycle state.
2. One-slot v1 overlay policy is now enforced both at control-level reservation and at runtime role-admission boundary.
3. Runtime status now exposes a concrete pane<->role binding observation (`overlay_bound_pane_id`) rather than only surface ids.

Preserved constraints:

1. Provider/topology authority remains outside compositor runtime.
2. Prototype overlay policy remains separate from long-term pane-hosting contract.
3. Native surfaces remain explicit `external/native`, never represented as `html`.

Not started in this pass:

1. DRM/KMS backend bring-up path.

## Validation run

Executed after implementation:

- `cargo check` (pass)
- `cargo test` (pass, 10/10)

New tests added for pass-5 bridge behavior:

- `runtime_overlay_bridge_transitions_follow_surface_lifecycle`
- `runtime_overlay_binding_expected_tracks_active_overlay_pane_state`
