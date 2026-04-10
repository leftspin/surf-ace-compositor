# Surf Ace Compositor — Adversarial Review (Overlay Alpha-Format Preference Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: atomic scanout format selection policy for primary vs overlay planes in `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Overlay transparency truth vs shared scanout-format preference

Result: **improved**.

Why:
- Scanout format selection is now plane-role aware.
- Primary plane still prefers `XRGB8888` first, preserving prior behavior for fullscreen composition.
- Overlay plane now prefers `ARGB8888` first, so overlay scanout keeps alpha semantics when supported by the plane.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- The change only affects preferred format choice among formats already reported by each plane.
- If a plane cannot do the preferred format, selection still falls back to the alternate supported format.
- Output-claim/reclaim, atomic commit failure handling, and runtime ownership semantics are unchanged.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- The slice improves concrete render/present behavior for the existing one-main + one-overlay v1 policy.
- Pane identity/role authority and overlay admission policy are unchanged.

## Nits / Next-Slice Cautions

1. This only improves preferred format selection; full cross-driver blend behavior still requires hardware validation.
2. Multi-plane behavior remains bounded to current primary+overlay policy.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test scanout_format_prefers` passes.
- `cargo test` passes.
- `select_preferred_scanout_format` now accepts `PlaneSelection` and applies role-specific preference lists.
- New tests verify:
  - primary prefers `XRGB8888` over `ARGB8888`
  - overlay prefers `ARGB8888` over `XRGB8888`

## Review Call

This slice closes a concrete runtime/hardware-facing gap after queued-framebuffer format telemetry: overlay scanout no longer accidentally prefers opaque scanout formats when alpha-capable formats are available.
