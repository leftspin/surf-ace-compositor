# Surf Ace Compositor — Adversarial Pass (2026-04-06)

## Scope

- `specs/surf-ace-compositor-v1-spec.md`
- `specs/surf-ace-compositor-spec-adversarial-review.md`
- current implementation in `src/`

## Real Issues First

### 1. Host failure kills the control plane, undermining bootstrap/recovery intent
Type: mismatch problem (spec requirement vs implementation behavior)

Evidence:
- Spec requires tiny bootstrap/recovery control path independent of Surf Ace usability: `specs/surf-ace-compositor-v1-spec.md:74-75`, `:211`.
- Host runtime process exits on host startup error: `src/main.rs:85-88`.
- Observed behavior in current environment: host preflight fails, then control socket is unavailable (`Connection refused`).

Why this is serious:
- When host startup fails (exactly when recovery controls matter), there is no process left to query status or apply recovery settings.
- This is a product-truth failure, not a polish issue.

What should change next:
- Keep compositor process and control server alive even when host runtime startup fails.
- Record host failure in runtime state and expose it over control (`get_status`) instead of exiting immediately.
- Add explicit retry/start-host control action or equivalent supervisor loop.

### 2. `HostDrm` is marked `Running` before any real DRM/KMS output ownership exists
Type: implementation problem (fake-progress risk)

Evidence:
- Runtime flips to `Running` immediately after preflight open of DRM device fds: `src/runtime.rs:331`.
- Host path currently includes session + udev + device open preflight, but no connector/CRTC/mode-set/output frame path.
- Spec Slice 5 requires DRM/KMS + GBM path and real output ownership: `specs/surf-ace-compositor-v1-spec.md:363-369`.

Why this is serious:
- Status semantics overclaim readiness and can hide that host compositor mode is not yet owning/producing outputs.
- This is the easiest place for implementation to go fake next (more preflight polish while calling it host runtime bring-up).

What should change next:
- Introduce a distinct host phase boundary (`preflight_ok` vs `running`) or equivalent so `running` means output ownership is live.
- Make next implementation slice first real KMS output claim (connector/CRTC selection + mode set) before adding more host preflight refinements.

### 3. Overlay role admission is pane-authoritative only at boolean level; no surface-to-request attestation
Type: implementation problem

Evidence:
- External/native request is process-spec/pane-authoritative (`switch_pane_to_external_native` + `SURF_ACE_PANE_ID`): `src/state.rs:196-237`.
- Runtime admits overlay if `runtime_overlay_binding_expected()` is true, then marks pane attached: `src/runtime.rs:804-819`, `src/state.rs:285-313`.
- No binding check ties accepted overlay surface to the specific requested process/pane identity.

Why this is serious:
- Any qualifying overlay candidate arriving at the right time can satisfy attach, even if it is not the requested pane process.
- This weakens the pane-authoritative external/native contract and can drift toward fake attachment semantics.

What should change next:
- Add an explicit runtime binding contract between requested overlay pane/process and accepted surface (token/identity handshake).
- Deny or quarantine overlay candidates that cannot satisfy that binding.

### 4. Adversarial-review readiness call is currently too optimistic vs implementation state
Type: spec problem (review-doc accuracy problem)

Evidence:
- Current review doc says QA/reality-pass-ready: `specs/surf-ace-compositor-spec-adversarial-review.md:61`.
- Findings 1 and 2 above are still product-significant and should block an unqualified readiness call.

Why this is serious:
- Creates false confidence and encourages progress theater.

What should change next:
- Update readiness call to conditional readiness:
  - architecture-ready: yes
  - implementation QA-ready: only after control-plane-on-host-failure and first real KMS ownership milestone are closed.

## Is the current implementation path still sound?

Yes, with a strict constraint:
- The path is sound **only if** the next pass pivots directly to (a) control-plane survivability on host startup failure and (b) first real DRM/KMS output-ownership milestone.
- If next passes keep refining preflight/status semantics without output ownership and failure-path control survivability, the path will drift fake.
