# Surf Ace Compositor Impl Pass 2 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Move from abstract core/control only to a real compositor runtime bring-up slice using Smithay `winit`, while preserving authority boundaries from pass 1.

## What was implemented

1. Added a Smithay runtime module (`src/runtime.rs`) with:
   - real `winit` backend initialization (`smithay::backend::winit::init::<GlesRenderer>()`)
   - event loop integration (`calloop` + `WinitEvent` source)
   - redraw cycle (`bind` + `submit`)
   - resize/input/redraw bookkeeping into shared compositor state
   - wayland display/listener skeleton (`Display` + `ListeningSocketSource`) wired into the same runtime loop
2. Bound runtime lifecycle into shared control state with explicit runtime status fields:
   - backend (`none`/`winit`)
   - phase (`inactive`/`starting`/`running`/`stopped`/`failed`)
   - window size
   - redraw/input counters
   - optional wayland socket name
3. Extended `serve` mode:
   - `--runtime none` (control/core only)
   - `--runtime winit` (real runtime bring-up path + control server sidecar)

## Adversarial findings during implementation

1. Initial runtime error type design duplicated `From<calloop::Error>` and broke compilation.
2. Several runtime setup points used `expect`, which could panic and hide control-plane diagnostics.

Fixes:

1. Reworked runtime errors to explicit string-backed variants with controlled `map_err`.
2. Removed `expect` from runtime source registrations in favor of typed error propagation.

## Spec-check verdict for this pass

Newly satisfied in this pass:

1. Slice-1 runtime proof point exists: Smithay runtime bring-up on `winit` is implemented as executable code.
2. Smithay event loop integration exists in runtime code (not only model/control abstractions).
3. Runtime behavior is now observable through control status (`runtime` section).
4. Authority model remains intact:
   - provider topology/pane geometry still authoritative input
   - runtime adds display policy/state only
5. Prototype policy remains separate from long-term pane-hosting contract:
   - single-overlay limitation remains in dedicated policy module
   - pane mode model remains per-pane and reversible

Still intentionally not done:

1. Real Wayland surface role assignment (fullscreen client + overlay client binding).
2. Actual xdg-shell handling and client surface composition.
3. Focus/input routing between fullscreen and overlay surfaces.
4. Output rotation applied at runtime backend level (state/control exists, backend transform work pending).

## Exit condition for pass 2

- Runtime bring-up is no longer abstract-only.
- A real Smithay `winit` path exists and compiles/tests in-repo.
- No authority-model regression introduced.
