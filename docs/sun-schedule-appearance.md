# Surf Ace Node Sun Schedule Appearance

T359 defines node-aware appearance publishing for appliance/kiosk mode. The compositor remains an environment signal publisher: it exposes `runtime.appearance` and schedule evidence, while Surf Ace and hosted apps own all rendering, toolbar color, CSS, and per-app styling.

## Requirements

- Each appliance node has an explicit sun schedule profile:
  - stable `nodeId`
  - IANA `timezone`
  - decimal latitude and longitude
- The schedule calculation uses the configured node location and timezone to compute real local sunrise and sunset for the evaluated local date.
- The appearance decision is:
  - `light` from local sunrise until local sunset
  - `dark` before sunrise and after sunset
  - `unknown` when no profile is present or the profile/evaluation cannot produce a concrete sunrise/sunset
- Status/control output must expose:
  - current `runtime.appearance`
  - `runtime.appearance_source`
  - the profile used for sun schedule evaluation
  - evaluation time, local date, sunrise, sunset, next transition, and reason
- Missing profile data is fail-closed and observable: appearance becomes `unknown` with reason `missing_profile`.
- Invalid timezone/location or polar no-rise/no-set cases are fail-closed and observable with an error-bearing schedule status.

## Source-Safe Slice

This source slice adds deterministic calculation and one-shot control application:

```json
{
  "type": "apply_sun_schedule_appearance",
  "profile": {
    "nodeId": "racter",
    "timezone": "America/Los_Angeles",
    "latitude": 37.7749,
    "longitude": -122.4194
  },
  "evaluatedAtUnixSeconds": 1718992800
}
```

When the request succeeds, status includes `runtime.sun_schedule` and updates `runtime.appearance` from the calculated schedule.

If `evaluatedAtUnixSeconds` is omitted, the compositor evaluates once using current system time. This is not autonomous scheduling; it is a one-shot source-safe primitive.

## Persistence Boundary

Autonomous sunup/sundown switching requires an approved product runtime path to invoke the evaluator at startup and at subsequent transitions. This source slice intentionally does not add or enable cron, launchd, systemd timers, restart policies, or any equivalent persistence.

The eventual persistent runner should use the same profile fields and control contract, then call the compositor at startup and at the published `nextTransitionUnixSeconds`. That deployment/runtime path remains blocked until Flynn explicitly authorizes persistence and deployment.
