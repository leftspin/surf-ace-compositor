# Surf Ace Compositor Impl Pass 7 — Adversarial Self-Review

Scope reviewed against cleaned spec:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Resume implementation against cleaned spec without adding doc-level implementation narration.

## Slice implemented

Host backend deterministic device-selection refinement (real path, no fake integration):

1. Host preflight now prefers seat primary GPU path when available.
2. Initial DRM device open order is deterministic (primary preferred, otherwise path-ordered).
3. Host runtime `host_primary_drm_path` selection is deterministic:
   - preferred primary when opened
   - otherwise lexicographically stable fallback across opened paths.

## Why this belongs to implementation (not spec)

- This is a concrete policy tactic inside host bring-up, not new product truth.
- Spec remains at requirement level: host path must be real, fail-closed, and observable.

## Adversarial checks

1. Does this violate authority boundaries? No.
2. Does this collapse `winit` and host modes? No.
3. Does this add fake backend wiring? No.
4. Is behavior testable? Yes (added unit tests for primary-path selection helper).

## Validation

- `cargo fmt` (pass)
- `cargo check` (pass)
- `cargo test` (pass, 12/12)
- host runtime smoke (`--runtime host`) still executes real preflight and fails closed under current permission limits
