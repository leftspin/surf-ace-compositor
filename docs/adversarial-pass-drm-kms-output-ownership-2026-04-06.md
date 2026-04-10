# Surf Ace Compositor — Adversarial Review (DRM/KMS Output-Ownership Slice, 2026-04-06)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host output-claim path in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress vs fake/theatrical

Result: **real progress**.

Why:
- Host path now performs an actual DRM/KMS output claim (`set_crtc`) with compositor-owned framebuffer before `running`.
- Claim path includes deterministic connector/CRTC/mode selection and device ordering, rather than status-only theater.

### 2) Does `running` honestly mean real host output ownership?

Result: **yes at transition boundary**.

Why:
- Host runtime enters `preflight_ready` after session/udev/device-open preflight.
- Host runtime enters `running` only after successful output claim.
- Claim failure remains explicit and fail-closed.

### 3) Connector/CRTC/mode selection + claim/reclaim semantics coherence

Result: **coherent and spec-aligned for first ownership milestone**.

Why:
- Device order is deterministic (preferred primary, then lexical fallback).
- Connected connectors with valid modes are selected; preferred mode ranking is explicit.
- CRTC routing is constrained via encoder possible-crtc filters.
- On output/device loss, reclaim is attempted; reclaim failure fails closed.

### 4) New spec drift / false confidence risks

Result: **no new hard drift found**.

Why:
- winit vs host split remains explicit.
- control/recovery path remains usable on startup failure.
- runtime does not silently downgrade host failure into winit behavior.

## Nits / Next-Slice Cautions

1. The slice is a first ownership milestone, not full host render pipeline.
   - Uses legacy KMS `set_crtc` + dumb framebuffer claim; GBM-driven composition and presentation is still pending.

2. Success-path QA still requires real hardware validation.
   - Current environment is headless, so passing checks are failure/recovery-path and unit/integration validation only.

3. Claimed-output identity is internal only.
   - Runtime status currently exposes ownership boolean/phase, not connector/crtc identifiers; useful but not mandatory for this slice.

## Evidence Snapshot

- `cargo test` passes (unit + integration, including host-failure survivability).
- Manual host checks in this environment show fail-closed startup with control path remaining reachable and retry-able.

## Review Call

This DRM/KMS output-ownership slice is **honest forward progress** toward real host compositor mode.
It does **not** appear theatrical, and it did **not** introduce new real product-truth failures in reviewed scope.
