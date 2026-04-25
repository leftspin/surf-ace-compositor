# Surf Ace Binding Compatibility Matrix - 2026-04-25

Scope: adversarial pass for the compositor/app binding seam after the RACTER Electron E2E. Surf Ace remains the provider and layout/content authority. The compositor only hosts native Wayland surfaces into Surf Ace-supplied pane rectangles and reports lifecycle/status.

## Covered Classes

| Class | Main app | Native pane | Binding authority | Status/cleanup expectation |
| --- | --- | --- | --- | --- |
| Direct compositor-spawned Wayland client | exact launched PID attaches | exact launched PID attaches | launched PID | attached state records client PID and surface id |
| Wrapper/launcher with descendant Wayland client | descendant PID attaches | descendant PID attaches | launched process lineage | parent exit after attach does not clear attached child truth |
| Terminal wrapper launching child UI/process | evidence may mismatch app_id/title | pane host attaches by launched terminal lineage | lineage first, app_id/title evidence only | child process is terminal payload, not binding authority |
| Native pane versus main app surface | full output main role | Surf Ace pane rectangle role | separate role maps | compositor does not create panes or choose geometry |
| app_id/title mismatch | recorded as evidence | recorded as not-required/evidence | never sole authority | mismatch is visible in status without denying a lineage match |
| Surface destroy/detach | clears main surface attachment | clears native pane surface id and returns to launching | attached PID | pane plan/content/binding stay present for recovery/rehost |
| Process exit | records exited and clears surface id | records exited and clears surface id | tracked PID | later `native_pane.host` relaunches absent/failed/exited panes |

## Live RACTER Evidence

Evidence root: `/tmp/surf-ace-electron-e2e-nativepane-20260425T060448Z`

The live E2E status showed:

- `output_rotation=deg90`
- `runtime.phase=running`
- `host_present_ownership=direct_gbm`
- `host_last_queued_present_source=direct_gbm`
- `host_active_connector_name=HDMI-A-3`
- Electron main app attached with `app_id="@surf-ace/electron"` and `main_app_surface_id=15`
- Surf Ace pane `1` rendered HTML content `ct_e2ehtml`
- Surf Ace pane `2` hosted native `top` through `foot --app-id surf-ace-pane-top top`
- Native pane status had `nativeHost.lifecycle.state=attached`, `surfaceId=3`, `contentId=ct_e2etop`, and `bindingId=sf_38ddb820a34e:2:ct_e2etop`

## Remaining Risk

The current lineage policy depends on `/proc` ancestry at toplevel arrival time. If a launcher exits before the real Wayland client creates its toplevel, the process tree can lose the ancestor link before the compositor can prove lineage. That class is not solved by app_id/title matching because those fields remain evidence, not authority. A future slice should add an explicit launch token/environment handshake for hosts that daemonize or detach before mapping a surface.

No compositor layout authority is introduced by this matrix: pane ids, rectangles, content ids, revisions, and process intent come from Surf Ace.
