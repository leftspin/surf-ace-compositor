# Launch Binding Reconciliation

Surf Ace owns pane identity, pane geometry, layout, and rendered content. The compositor owns only the native-surface hosting boundary that Surf Ace calls into.

For compositor-launched main apps and native pane hosts, surface binding is reconciled from the launched request and Wayland client credentials first. When the arriving toplevel's client PID matches the compositor-spawned process PID, the surface is eligible for the requested main-app or pane-host role. `app_id` and title are recorded as binding evidence in status, but they are not the sole authority for attachment.

The launch-intent contract remains a process spec plus binding identity. For shorthand launches, the compositor still injects a generic Surf Ace class/app-id where supported so cooperative clients can identify themselves. Non-cooperative clients such as Ghostty may expose their own Wayland app id instead; that mismatch is reported as evidence, not treated as a denial when process lineage matches.

Detached launchers are covered when the eventual Wayland client inherits the compositor-generated `SURF_ACE_COMPOSITOR_LAUNCH_TOKEN` and the compositor can read that client's `/proc/<pid>/environ`. PID/descendant lineage remains the first authority; token evidence only augments the launched request when ancestry is gone. `app_id` and title remain evidence and cannot attach a surface by themselves.

Current limitation: no Wayland core protocol exposes arbitrary client environment variables. If procfs environment reads are unavailable, or the launched program scrubs the token before creating the Wayland client, detached clients fall back to the existing PID/descendant policy and may remain `exited`/unattached until relaunched with a cooperative wrapper.

## Native Pane Host Binding

`bind_native_pane_host_surface` is the pane-host binding control primitive for this phase. It accepts an arriving Wayland client PID plus optional app/title evidence and reconciles it to the pane whose native host launch state is waiting for that PID. The response is the normal status snapshot; Surf Ace should read the pane's `external_native_state` and `external_native_binding_evidence`.

This remains a compositor hosting primitive, not a layout primitive. Surf Ace still supplies pane ids and rectangles through the native pane host plan, and the compositor only binds launched native surfaces to those provider-owned pane records.

See `docs/native-pane-control-contract-2026-04-24.md` for the Surf Ace bridge-facing request names, socket discovery, idempotency, release policy, and status shape.
