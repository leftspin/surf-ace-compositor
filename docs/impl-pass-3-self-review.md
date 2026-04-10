# Surf Ace Compositor Impl Pass 3 — Adversarial Self-Review

Scope reviewed against:

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`

## Goal for this pass

Advance from runtime skeleton into real surface/runtime behavior:

- xdg-shell client handling
- explicit main-vs-overlay toplevel role policy
- focus/input routing skeleton
- backend-applied rotation in render path

## What was implemented

1. Smithay wayland protocol handlers in runtime (`src/runtime.rs`):
   - compositor handler
   - xdg-shell handler
   - shm handler
   - seat handler
2. Real xdg toplevel runtime behavior:
   - first toplevel -> `main_app` role (fullscreen configure)
   - second toplevel -> `overlay_native` role
   - further independent toplevels -> denied via `send_close`, with denied counter
3. Real render path for role-bound surfaces:
   - render main and overlay surface trees
   - send frame callbacks for role surfaces
4. Focus/input routing skeleton:
   - keyboard focus resolves to active target (`main_app` or `overlay_native`)
   - control path can set/clear requested runtime focus target
   - runtime reconciles requested focus with live role surfaces
5. Rotation application in backend render path:
   - `OutputRotation` mapped to Smithay `Transform` each redraw
   - transform passed into renderer render pass

## Adversarial findings during implementation

1. Initial implementation used wrong winit input backend type and failed compile.
2. Initial render path borrowed backend mutably twice (`bind` + `submit`) and failed borrow check.
3. An import mistakenly used `with_surface_tree_downward` from wrong module.

Fixes:

1. switched input handling to generic `InputEvent<B: InputBackend>`.
2. scoped render pass borrow, then submitted after scope end.
3. moved `with_surface_tree_downward` import to `smithay::wayland::compositor`.

## Spec-check verdict for this pass

Newly satisfied in this pass:

1. Real xdg-shell client surface handling exists in runtime code.
2. Explicit runtime role policy for fullscreen main app vs overlay-native exists and is enforced.
3. Additional independent toplevel windows are explicitly denied in v1 runtime policy.
4. Runtime focus routing between main and overlay paths exists as executable skeleton.
5. Rotation is now applied in backend render path, not only stored in control state.

Preserved constraints:

1. Provider/topology authority remains outside compositor runtime.
2. Prototype role policy remains separate from long-term pane-hosting model.
3. No fake HTML/native collapse introduced.

Still intentionally not done:

1. deterministic Surf Ace-main binding based on production app identity/bootstrap contract (current fallback is slot-order + app-id hint only)
2. full popup positioning/clipping policy implementation for all transient cases
3. complete pointer routing semantics with geometry hit-testing and seat grabs
4. DRM/KMS host backend path and real-output ownership

## Exit condition for pass 3

- Runtime now handles real xdg-shell surfaces with explicit role policy and render path.
- Focus/rotation behaviors are represented in executable runtime code.
- No authority-model regression introduced.
