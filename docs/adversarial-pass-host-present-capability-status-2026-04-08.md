# Surf Ace Compositor — Adversarial Review (Host Present-Capability Status Slice, 2026-04-08)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- implementation focus: host runtime status truth for present-path capability in `src/model.rs`, `src/state.rs`, and `src/runtime.rs`

## Real Issues First

No new product-truth failures were found in this slice.

## Focus Checks

### 1) Runtime/control truth for host present capabilities

Result: **improved**.

Why:
- Runtime status now explicitly reports host present ownership mode (`none`, `dumb`, `direct_gbm`), whether atomic commit is enabled, and whether an overlay plane is present-capable on the claimed pipeline.
- Host startup and reclaim claim paths now synchronize these fields from the claimed output pipeline instead of leaving control/status blind to present capability state.
- Claim-loss paths now explicitly clear present capability status fail-closed when output ownership is dropped.

### 2) Running/recovery truth and fail-closed behavior

Result: **preserved**.

Why:
- No runtime phase transition semantics changed.
- Existing reclaim/failure logic is unchanged; this slice adds status synchronization and fail-closed clearing around those existing transitions.
- `mark_runtime_starting`, preflight, stopped, and failed transitions now clear present capability fields to avoid stale claims.

### 3) Spec alignment and authority boundaries

Result: **aligned**.

Why:
- No pane identity/geometry/role authority behavior changed.
- This is explicit runtime observability plumbing for the host output/present authority already in scope.
- It supports adversarial validation and control-path clarity without broadening product claims.

## Nits / Next-Slice Cautions

1. This slice reports capability state, not per-frame “actually used this frame” telemetry.
2. Hardware validation is still required to prove real driver behavior for the exposed capabilities.

## Evidence Snapshot

- `cargo fmt --all` passes.
- `cargo test` passes.
- `RuntimeStatus` now includes explicit host present-capability fields.
- `HostBackendState::claimed_present_capabilities` derives capability truth from claimed output + pipeline state.
- Startup/reclaim/claim-loss paths synchronize or clear present-capability fields through shared runtime state.

## Review Call

This slice closes a concrete observability gap: control/runtime status now tells the truth about host present-path capabilities instead of forcing inference from logs or source inspection.
