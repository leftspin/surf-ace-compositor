# Surf Ace Compositor — Adversarial Cycle (2026-04-06, Round 2)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- current implementation in `src/`

## Real Issues (Cycle Result)

No remaining product-truth failures found in this cycle.

## Real Issues Closed Since `docs/adversarial-pass-clean-spec-impl-2026-04-06.md`

1. Host startup failure no longer kills the bootstrap/recovery control path.
   - Code: `src/main.rs` now keeps control server alive after `run_host` error and records failure in runtime status.
   - Runtime evidence: `serve --runtime host` in this headless environment now returns `phase=failed` over control while process stays alive.

2. Host `running` overclaim removed.
   - Code: `src/model.rs` adds `RuntimePhase::PreflightReady` and `host_output_ownership`.
   - Code: `src/runtime.rs` host path now reports preflight readiness after seat/udev/device acquisition; it does not report `running` before real output ownership exists.

3. Overlay attach now requires explicit surface-to-request process attestation.
   - Code: `src/runtime.rs` gates overlay admission on Wayland client PID credentials.
   - Code: `src/state.rs` requires PID-matching attach/detach transitions against the active pane-bound external/native request.

4. Runtime-mode truth tightened.
   - Code: `src/main.rs` now sets `host_mode_active` from selected runtime mode (`host` only), avoiding mode-collapse signaling in `none`/`winit`.

## Nits Only

1. Add a control action for host-runtime retry/restart after startup failure so recovery does not require process restart.
   - This is operational ergonomics; control/status truth is already preserved.

2. Add a dedicated integration test for host startup failure survivability (socket stays reachable + `phase=failed` surface) to lock in the behavior currently verified manually.

3. Overlay PID attestation is strict and may reject launcher/wrapper chains where the Wayland client PID differs from the original spawned PID.
   - Current behavior is fail-closed and honest.
   - Future improvement can add an explicit binding token protocol for wrapper-heavy targets.

## Soundness Call

Current implementation path is sound and no longer fake-progressing on the targeted knives:
- bootstrap/recovery control remains usable when host startup fails
- `running` is no longer used for host preflight theater
- pane-authoritative overlay attachment now requires process-level attestation
