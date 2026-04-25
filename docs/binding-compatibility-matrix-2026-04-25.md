# Surf Ace Binding Compatibility Matrix - 2026-04-25

Scope: adversarial pass for the compositor/app binding seam after the RACTER Electron E2E. Surf Ace remains the provider and layout/content authority. The compositor only hosts native Wayland surfaces into Surf Ace-supplied pane rectangles and reports lifecycle/status.

## Covered Classes

| Class | Main app | Native pane | Binding authority | Status/cleanup expectation |
| --- | --- | --- | --- | --- |
| Direct compositor-spawned Wayland client | exact launched PID attaches | exact launched PID attaches | launched PID | attached state records client PID and surface id |
| Wrapper/launcher with descendant Wayland client | descendant PID attaches | descendant PID attaches | launched process lineage | parent exit after attach does not clear attached child truth |
| Wrapper/launcher that daemonizes before mapping | launch token can attach detached client when inherited env is visible | launch token can attach detached client when inherited env is visible | compositor-generated launch token plus PID lineage | launcher may show `exited` until the detached client maps and proves the token |
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

## Launch Token Hardening

The compositor now emits `SURF_ACE_COMPOSITOR_LAUNCH_TOKEN` into compositor-spawned main-app and native-pane host processes. For native panes the same launch environment also carries `SURF_ACE_PANE_ID`, `SURF_ACE_NATIVE_PANE_CONTENT_ID`, `SURF_ACE_NATIVE_PANE_BINDING_ID`, and `SURF_ACE_NATIVE_PANE_REVISION` when those Surf Ace-owned values are present.

At toplevel arrival, binding still tries launched PID/descendant lineage first. If lineage is not provable, the runtime reads `/proc/<client-pid>/environ` for the token and accepts a match as launch evidence. This allows a launcher that exits before mapping to leave the pane/main-app lifecycle as `exited` temporarily, then attach the later detached Wayland client if that client inherited the compositor token.

`app_id` and title remain evidence only. Status binding evidence may include `launchToken: "matched"`, `"mismatched"`, `"missing"`, or `"unavailable"`; the raw token is not serialized.

Exact limitation: this token path depends on the compositor being able to read the arriving client process environment through `/proc/<pid>/environ`. If the client scrubs its environment, changes credentials, hides procfs, or otherwise makes environ unreadable, the token result is `missing` or `unavailable` and the compositor falls back to PID/descendant lineage. No Wayland core protocol exposes arbitrary client environment directly.

No compositor layout authority is introduced by this matrix: pane ids, rectangles, content ids, revisions, and process intent come from Surf Ace.
