# Surf Ace Compositor Spec — Adversarial Review

## Verdict

The spec direction remains ==sound==, and the spec is now ==cleaner source-of-truth guidance with less implementation narration==.

This revision preserves the pass 4-6 truth that belongs in spec while compressing code-shaped detail:

- deterministic main/overlay role policy is now explicit
- overlay-role admission is explicitly subordinate to pane state/lifecycle truth
- popup/transient ownership policy is explicit in the subsystem contract
- development `winit` runtime vs true host DRM/KMS runtime is explicitly separated
- host runtime fail-closed behavior (no silent fallback) is now explicit

## Strong parts

1. ==Authority model remains correct and explicit.==
   - Surf Ace/provider still own pane identity, geometry, topology, and pane mode truth.
   - Compositor remains display/output/input/runtime policy authority, not topology authority.

2. ==Prototype policy vs long-term contract is preserved.==
   - one fullscreen + one overlay remains explicit v1 prototype policy
   - long-term per-pane dynamic hosting contract remains explicit and separate

3. ==Runtime/product bridge is now better specified.==
   - role lifecycle reconciliation with pane lifecycle truth is now in spec
   - bridge observability expectations are now explicit enough for QA targeting

4. ==Host backend path expectations are now explicit.==
   - host preflight includes session acquisition, seat-scoped DRM discovery, and real device-open attempts
   - host mode activation/failure behavior is explicit and not conflated with `winit`

5. ==Spec/implementation boundary is cleaner.==
   - implementation-tactic prose was compressed back into architecture-level requirements
   - detail that belongs in impl/review docs is no longer bloating core guidance

## Adversarial findings (remaining weak or unresolved)

### 1. Provider/compositor wire schema is still intentionally under-specified
The spec now describes required semantics more clearly, but exact wire/API shape (including versioning and compatibility policy) remains open.

### 2. First host KMS policy is still open at implementation-policy level
The spec now requires real host bring-up behavior, but still leaves connector/CRTC/device selection and recovery policy as an unresolved implementation decision.

### 3. QA acceptance matrix is implied, not yet explicitly enumerated
The spec now contains stronger success criteria and subsystem constraints, but does not yet enumerate a concrete QA matrix across:
- host-permission-success vs permission-denied paths
- device hotplug/remove scenarios
- deterministic role binding edge cases

## What changed from the previous adversarial review

1. Prior editorial concerns about prototype-vs-contract ambiguity were substantially improved.
2. Control-plane narrowness remained intact and is still correctly constrained.
3. The largest previous gap (explicit host backend path semantics) is now addressed at spec level.

## Readiness call

==Architecture-ready: yes.==

==QA/reality-pass-ready against implementation: yes, with bounded unresolved items.==

==Fully implementation-closed: not yet.==

The remaining gaps are concentrated in wire-shape finalization and host KMS policy details, not in core architecture direction.
