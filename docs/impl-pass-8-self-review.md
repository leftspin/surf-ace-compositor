# Surf Ace Compositor — Impl Pass 8 Self-Review

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- runtime host path changes in `src/runtime.rs`

## Real Issues First

No new product-truth regressions found in this pass.

## What was adversarially checked

1. `running` semantics are still honest.
   - Host runtime now reaches `running` only after successful DRM/KMS output claim (`set_crtc` with compositor-owned dumb framebuffer).
   - Host startup that cannot claim output returns explicit failure and stays recoverable through control path.

2. No fake preflight theater.
   - Preflight remains explicit (`preflight_ready`) and does not imply ownership.
   - Ownership claim is a separate hard gate before `running`.

3. Control/recovery truth remains intact.
   - Existing host failure survivability test still passes.
   - Existing restart control path still works.

4. Deterministic first output policy exists.
   - Device ordering uses preferred-primary then lexical fallback.
   - Connector/CRTC/mode selection is deterministic and explicit.

## Nits / follow-up (not blockers for this slice)

1. Host claim currently uses legacy KMS `set_crtc` + dumb buffer as the first ownership milestone; it is intentionally not yet a full GBM render pipeline.
2. Runtime status does not yet expose claimed connector/crtc identifiers (debugging ergonomics only).

## Conclusion

Pass 8 meets the stated slice goal: first honest step from host preflight into real DRM/KMS output ownership, while preserving fail-closed startup and control-plane recovery behavior.
