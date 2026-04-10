# Surf Ace Compositor — Adversarial Review (Atomic/Direct-Present Substrate Slice, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host DRM claim/commit path in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Is this real progress beyond legacy `set_crtc` + page-flip assumptions?

Result: **yes**.

Why:
- Host claim plan now probes optional atomic capability and property/primary-plane viability from real DRM object state (connector/crtc/plane property handles).
- Startup ownership claim can now execute via real atomic modeset commit (`CRTC_ID`, `ACTIVE`, `MODE_ID`, primary-plane state) when supported.
- Runtime frame queue can now use atomic commit (`NONBLOCK|PAGE_FLIP_EVENT`) for primary-plane framebuffer updates when atomic startup claim was actually established.
- Legacy `set_crtc` / `page_flip` remains explicit fallback when atomic substrate is unavailable or fails during startup bring-up.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- `running` still only occurs after successful DRM output ownership claim.
- Atomic path is only activated after successful startup atomic claim; there is no optimistic "atomic enabled" claim before commit success.
- If direct-present ownership was established, render path still fails closed on direct-render loss rather than silently degrading.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- No second topology authority was introduced.
- `winit` and host DRM paths remain separate.
- This slice is backend lifecycle substrate only; prototype overlay policy and long-term pane contract separation remains intact.

## Nits / Next-Slice Cautions

1. Atomic path is still single-plane primary-plane management only; it is not full drm-compositor transaction orchestration.
2. Claim/reclaim across hotplug/device-loss still stops runtime rather than attempting in-process atomic rebind.
3. Hardware validation is still required across real drivers for mixed fallback behavior (atomic-capable vs legacy-only devices).

## Evidence Snapshot

- `cargo fmt` and `cargo test` pass.
- Atomic claim-plan probe and property mapping are in `build_atomic_claim_plan`.
- Startup claim uses `claim_output_with_atomic_modeset` when available.
- Ongoing present queue uses `queue_atomic_frame_commit` when atomic claim state is active.

## Review Call

This is an honest substrate step toward drm-compositor-grade claim+commit management without pretending full atomic plane orchestration.
