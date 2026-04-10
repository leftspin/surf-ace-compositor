# Surf Ace Compositor Impl Pass 4 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Tighten runtime behavior toward product truth:

- deterministic Surf Ace-main binding path
- stronger popup/transient policy
- fuller pointer/focus routing semantics

## What was implemented

1. Deterministic main-surface binding path:
   - runtime now classifies xdg toplevels by explicit `main_app_match_hint`
   - default hint is `surf-ace`
   - configurable via control request: `set_runtime_main_app_match_hint`
   - toplevels with missing identity are queued as pending until app-id/title information is available
2. Role assignment policy tightened:
   - `main_app` assignment is no longer mostly slot-order fallback
   - non-main surfaces can become overlay candidates; extra independent toplevels remain denied
3. Stronger popup/transient policy:
   - popup accepted only if parent belongs to known role owner
   - popup geometry is constrained to owner bounds via positioner unconstrained-geometry logic
   - popup ownership is tracked and used for focus and rendering
4. Fuller pointer/focus routing semantics:
   - pointer absolute motion routed to popup/overlay/main ownership hierarchy
   - pointer button events update keyboard focus to surface under pointer
   - pointer axis events forwarded through seat pointer pipeline
   - keyboard focus target now maps through popup ownership to owning role target

## Adversarial findings during implementation

1. Input path initially had duplicate processing in runtime loop.
2. Popup rendering used wrong coordinate-kind type for location.
3. Potential role confusion existed when app-id update arrived for current overlay.
4. Pointer motion was initially forwarded with global coordinates even for overlay/popup targets, which is incorrect for per-surface pointer semantics.

Fixes:

1. collapsed to single input processing path per event.
2. converted popup render location to physical-compatible tuple.
3. guarded against accidental overlay promotion in app-id update handling.
4. updated hit-testing return coordinates to surface-local coordinates for overlay and popup targets.

## Spec-check verdict for this pass

Newly satisfied in this pass:

1. Main-surface binding is now explicit and configurable, not predominantly first-slot fallback.
2. Popup/transient handling is more explicitly owner-bound with in-owner geometry constraints.
3. Pointer/focus routing has concrete role-aware motion/button/axis behavior.

Preserved constraints:

1. Provider/topology authority remains outside compositor runtime.
2. Prototype overlay policy remains distinct from long-term pane-hosting contract.
3. Native surfaces remain explicit, never represented as `html`.

Not started in this pass:

1. DRM/KMS host backend runtime structure.

Rationale:

- A clean start on DRM/KMS would require introducing additional backend plumbing that cannot be exercised in current headless context without risking speculative or fake wiring. Deferred to pass 5.

## Exit condition for pass 4

- Runtime role/popup/focus behavior is more deterministic and better policy-enforced.
- No authority-boundary regression introduced.

## Validation run

Executed after implementing/fixing pass-4 slice:

- `cargo check` (pass)
- `cargo test` (pass, 8/8)
- control-path smoke with `--runtime none`:
  - `set_runtime_main_app_match_hint`
  - `set_runtime_focus_target` / `clear_runtime_focus_target`
  - `get_status`
  - responses reflected expected runtime status mutations
