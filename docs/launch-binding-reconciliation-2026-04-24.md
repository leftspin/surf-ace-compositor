# Launch Binding Reconciliation

Surf Ace owns pane identity, pane geometry, layout, and rendered content. The compositor owns only the native-surface hosting boundary that Surf Ace calls into.

For compositor-launched main apps and native pane hosts, surface binding is reconciled from the launched request and Wayland client credentials first. When the arriving toplevel's client PID matches the compositor-spawned process PID, the surface is eligible for the requested main-app or pane-host role. `app_id` and title are recorded as binding evidence in status, but they are not the sole authority for attachment.

The launch-intent contract remains a process spec plus binding identity. For shorthand launches, the compositor still injects a generic Surf Ace class/app-id where supported so cooperative clients can identify themselves. Non-cooperative clients such as Ghostty may expose their own Wayland app id instead; that mismatch is reported as evidence, not treated as a denial when process lineage matches.

Current limitation: this slice proves direct spawned-process PID reconciliation. It does not yet prove descendant process lineage for launchers that fork/exec a separate Wayland client and exit. Those cases must remain explicit in status as launching/exited/failed until a process-tree or token-based attestation path is added.
