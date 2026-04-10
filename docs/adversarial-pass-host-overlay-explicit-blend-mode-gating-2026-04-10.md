# Surf Ace Compositor — Adversarial Review (Overlay Explicit Blend-Mode Gating Slice, 2026-04-10)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: fail-closed overlay-plane eligibility when blend-mode capability is implicit/unknown in `src/runtime.rs`

## Real Issues First

No new product-truth failures were introduced in this slice.

## Focus Checks

### 1) Overlay-plane alpha truth vs missing blend-mode property

Result: **improved**.

Why:
- Overlay-plane composition now requires explicit `pixel blend mode` property support with an alpha-capable enum (`premultiplied` or `coverage`).
- If the overlay plane does not expose `pixel blend mode`, overlay routing is disabled fail-closed instead of assuming driver defaults.
- Runtime present capability truth now reflects this gate via `host_overlay_plane_capable=false` on such hardware.

### 2) Running/recovery behavior under stricter gating

Result: **preserved and honest**.

Why:
- Host runtime still reaches `phase=running` with direct GBM atomic present ownership.
- Overlay-plane split is disabled only when alpha-safe composition cannot be proven, while primary-plane composition remains active.
- No new failure/recovery loops were introduced.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- The slice narrows a hardware-facing claim and keeps v1 behavior fail-closed.
- Pane authority, role admission policy, and topology boundaries are unchanged.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test` passes.
- Visible host verification run on racter:
  - evidence bundle: `/tmp/surf-ace-visible-verify-20260410T120339Z`
  - `summary.txt`: `phase=running`, `wayland_socket=wayland-1`
  - `status_running.json`: `host_present_ownership=direct_gbm`, `host_atomic_commit_enabled=true`, `host_overlay_plane_capable=false`
  - compositor log evidence:
    - `host backend overlay plane on /dev/dri/card1 is missing pixel blend mode property; forcing fail-closed overlay-plane disable`
    - `host backend overlay plane on /dev/dri/card1 lacks alpha-safe blending controls; disabling overlay plane routing for this output`

## Review Call

This slice closes the remaining optimistic gap in overlay-plane blend-control truth: overlay routing is no longer treated as available when alpha-safe blend semantics are not explicitly representable by the driver plane properties.
