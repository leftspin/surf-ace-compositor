# Surf Ace Compositor — Adversarial Cycle (2026-04-06, Round 3)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- current implementation in `src/` and `tests/`

## Real Issues First

No new real product-truth failures found.

## Nit Closure Verification

1. `start_host_runtime` control action exists and is wired to real host runtime retry/start behavior.
   - `ControlRequest::StartHostRuntime` is implemented and host-mode gated.
   - Host runtime starts through an explicit runtime-control command path, including initial bootstrap start and later control-triggered retries.

2. Host-startup-failure survivability is locked by integration test.
   - Added `tests/host_failure_survivability.rs`.
   - Test enforces: control socket remains reachable, runtime phase is `failed`, restart action is accepted, process remains alive.

3. Wrapper/launcher-chain attestation note is explicit and behavior remains fail-closed.
   - Current attach policy still requires process-level attestation and rejects mismatches.
   - Follow-up remains: add token-based binding path for wrapper chains without weakening fail-closed semantics.

## Implementation Resumed Slice

After nit closure, implementation resumed on runtime/product observability for host lifecycle:

- runtime status now tracks host start attempts and trigger source (`bootstrap` vs `control_retry`)
- status makes restart intent auditable via control without log scraping

This is aligned with spec requirements that runtime status and backend readiness remain reconcilable with control/recovery truth.

## Result

Adversarial result remains ==nits-only==.
