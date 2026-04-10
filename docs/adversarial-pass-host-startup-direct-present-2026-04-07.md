# Surf Ace Compositor — Adversarial Review (Startup Toward Direct Present, 2026-04-07)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: startup claim/present path in `src/runtime.rs`

## Real Issues First

No new real product-truth failures were found in this slice.

## Focus Checks

### 1) Real progress vs fake/theatrical

Result: **real progress**.

Why:
- Startup claim now attempts a direct GBM-present modeset first by priming and using a direct GBM framebuffer for initial `set_crtc` when supported.
- Dumb startup buffers are no longer mandatory at claim time; they are allocated lazily only when fallback path is actually needed.
- Direct-vs-dumb flip source tracking is explicit and drives the correct ring swap on pageflip completion.

### 2) Running/recovery truth and fail-closed semantics

Result: **preserved**.

Why:
- `running` still means real output ownership and remains gated after successful host claim/modeset.
- Queue/event failures still fail runtime closed.
- If direct startup cannot be used, claim path explicitly falls back to dumb modeset without overclaiming direct ownership.

### 3) Control-plane survivability

Result: **preserved**.

Why:
- Host startup failure survivability integration test still passes.
- Retry/start control behavior remains unchanged.

### 4) Spec alignment

Result: **aligned**.

Why:
- No second topology authority introduced.
- Runtime-mode split (`winit` vs host) remains explicit.
- Prototype overlay policy vs long-term pane-hosting contract remains untouched.

## Nits / Next-Slice Cautions

1. Direct startup modeset is now preferred but still contingent on backend support; dumb fallback remains necessary for unsupported paths.
2. Startup path still relies on legacy `set_crtc` claim flow rather than full atomic/drm-compositor present management.
3. Full direct-path default still needs broader real-hardware validation and stronger direct-path-only confidence before removing fallback from default behavior.

## Evidence Snapshot

- `cargo fmt && cargo test` passes.
- Startup claim path now primes direct GBM framebuffer and can modeset directly without pre-allocating dumb buffers.

## Review Call

This slice is an honest step from optional direct present toward direct GBM-present ownership from startup.
No new real product-truth failures were introduced in reviewed scope.
