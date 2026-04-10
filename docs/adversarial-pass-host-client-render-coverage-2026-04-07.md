# Surf Ace Compositor — Adversarial Review (Client/Render Coverage Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host Wayland scene capture and fallback buffer path in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress toward broader client/render coverage

Result: **yes**.

Why:
- The host fallback now walks the actual surface tree for the main and overlay panes and pulls in each committed buffer, so subsurfaces are no longer silently ignored when the fallback pipeline is active.
- Each buffer commit records whether the buffer came from wl_shm, a linux-dmabuf/EGL source, or another type, which lets the fallback path avoid pretending it can blit unsupported buffers while still keeping their metadata for future inspection.
- `compose_host_scene` now honors the new surface kind metadata, only attempting CPU blits for the wl_shm surfaces while logging when DMABUF/EGL surfaces are skipped, keeping the runtime honest about what client buffers are actually rendered without dropping the rest silently.
- The renderer now calls `import_surface_tree` on each main, overlay, and popup surface before composing, which lets DMABUF/EGL buffers be imported as GPU textures instead of being silently skipped by the fallback path.
- The renderer now captures those imported surfaces and feeds them into the GBM/atomic present path so DMABUF/EGL textures are actually drawn and submitted to real scanout buffers instead of being merely inspected.

### 2) Running/recovery truth and authority boundaries

Result: **preserved**.

Why:
- No new control or compositor state was introduced; host running state still requires real DRM ownership and fails closed if the direct render path fails.
- The overlay policy remains pane-authoritative: overlay surfaces still have to be authenticated via the PID attachment logic before their buffers are considered in the atomic layout path.
- The fallback scene composer only augments local drawing when necessary and never claims to handle DMABUF/EGL buffers unless the direct rendering pipeline already handles them.

### 3) Spec alignment

Result: **aligned**.

Why:
- The prototype overlay policy vs long-term pane contract separation remains intact; new surface collection logic simply mirrors the existing pane geometry.
- Development `winit` mode and the true DRM path stay distinct; these coverage improvements only touch the DRM host runtime.
- Authority boundaries are untouched because the compositor still relies on provider-supplied pane rectangles for layout data.

## Nits / Next-Slice Cautions

1. Fallback composition still can only CPU-blit wl_shm buffers; linux-dmabuf/EGL surfaces are logged but left for the direct GPU path.
2. The logging is intentionally verbose to document what is skipped; hardware validation is still required to confirm that the direct renderer path covers all real dmabuf/EGL clients.
3. Subsurface geometry accuracy relies on the cached subsurface location; any future compositor features that mutate that layout should keep this path in sync.
4. Import errors are logged and automatically disable the renderer’s capture path, ensuring the compositor only claims DMABUF/EGL coverage it can actually present.

## Evidence Snapshot

- `cargo test` passes.
- `collect_surface_tree_surfaces` now walks the toplevel and overlay trees and adds their subsurfaces to the fallback scene list.
- Each buffer commit records a `SurfaceBufferKind` so `compose_host_scene` only blits wl_shm buffers and logs when dmabuf/EGL surfaces are skipped.
- The renderer now captures the imported surface trees and reuses that capture in the GBM/atomic present pipeline, forcing DMABUF/EGL clients to go through the real render/present path instead of ending at inspection.
- The fallback snapshot now records a concrete `Modifier` in `SurfaceDmabufInfo`, matching the metadata returned by `smithay::wayland::dmabuf::get_dmabuf` and keeping the compile path honest.

## Review Call

This slice broadens the client/render coverage truthfully and without fake buffer claims. No new spec drift was introduced.
