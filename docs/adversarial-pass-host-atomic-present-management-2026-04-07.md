# Surf Ace Compositor — Adversarial Review (Atomic Present Management Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: multi-plane atomic/direct-present layout handling in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress beyond single-plane primary-plane atomic/direct-present

Result: **yes**.

Why:
- The atomic commit path now understands plane layouts instead of reusing the primary plane geometry for every plane.
- An `AtomicPlaneLayout` abstraction describes CRTC and source rectangles while the runtime derives the overlay layout directly from the overlay rectangle computed by the Wayland host state.
- Atomic commits disable overlay planes when no overlay surface is attached and enable them with truthful geometry only when the overlay pane is active, so the host no longer fakes multi-plane orchestration by waving a single plane at overlay content.

### 2) Running semantics and fail-closed behavior

Result: **preserved**.

Why:
- Host mode still transitions to `running` only after committing real DRM ownership; there is no additional optimism in layout management.
- If the direct path fails, the code still falls back to dumb buffers and the runtime keeps the control/recovery plumbing alive rather than pretending overlays are active.
- Overlay plane enablement is tied to explicit overlay role attachment and respects the pane-authoritative lifecycle described in spec.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- The overlay plane remains subordinate to the long-term pane-hosting contract; layout activation maps to the overlay rectangle produced by the pane-aware runtime state.
- Runtime role policy continues to be deterministic: overlay plane geometry is never assumed unless the overlay surface has been authenticated through the ribbon-of-trust PID binding.
- Development `winit` mode remains separate; these atomic/direct-present changes only affect the DRM host path.

## Nits / Next-Slice Cautions

1. This slice still does not provide full atomic transaction orchestration (plane property snapshots, multi-plane commit sequencing, etc.); it is still a substrate step toward that truth.
2. Hardware validation across different kernels/drivers is still required to confirm that overlay layout disabling/enabling behaves consistently on real GPUs.
3. Deeper overlay geometry control (scaling, rotation, clipping) remains future work once the present management path has solid ground.

## Evidence Snapshot

- `cargo test` passes and `AtomicPlaneLayout` is now used for both the initial atomic modeset claim and subsequent frame commits.
- `queue_atomic_frame_commit` derives overlay layout from `wayland_state.overlay_rect()` and only supplies plane properties when an overlay surface exists.
- Direct-present ownership still depends on explicit pipeline layout state (`atomic_commit_state`) and adopts the overlay layout only after physically composing the overlay surface buffer.

## Review Call

This slice honestly expands the atomic/direct-present path beyond the single-plane primary-plane assumption. No new spec drift or fake claims were introduced.
