# Surf Ace Compositor Status Evidence

Date: 2026-04-12
Scope: preserved runtime-status evidence for the verified RACTER tty4 rotation run
Status: copied from verified RACTER artifacts; no merge performed, no deploy performed

## Evidence Set

Preserved status directory:

- `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses`

These JSON files are preserved copies of the verified per-rotation status outputs originally written on RACTER to:

- `/tmp/surf-ace-deg0-status.json`
- `/tmp/surf-ace-deg90-status.json`
- `/tmp/surf-ace-deg180-status.json`
- `/tmp/surf-ace-deg270-status.json`

The records came from the same verified tty4 Ghostty + `zsh` direct-present path referenced by [verification-package-racter-tty4-2026-04-12.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12.md).

## What The Status Files Prove

Each preserved JSON contains the full runtime status record for one verified rotation. The key proof fields used in this lane are:

- `status.output_rotation`
- `status.runtime.host_present_ownership`
- `status.runtime.host_last_queued_present_source`
- `status.runtime.input_event_count`
- `status.runtime.main_app_match_hint`

For all four preserved rotations, those fields show:

- the requested rotation was active in the status snapshot
- the live present path was `direct_gbm`
- the last queued present source was `direct_gbm`
- `main_app_match_hint` was `ghostty`
- `input_event_count` was nonzero at capture time, which is the runtime-side evidence preserved here that input was live during the verified run

## Rotation Summary

- [deg0-status.json](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg0-status.json)
  - output rotation: `deg0`
  - host present ownership: `direct_gbm`
  - last queued present source: `direct_gbm`
  - input event count: `43`
  - main app match hint: `ghostty`

- [deg90-status.json](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg90-status.json)
  - output rotation: `deg90`
  - host present ownership: `direct_gbm`
  - last queued present source: `direct_gbm`
  - input event count: `39`
  - main app match hint: `ghostty`

- [deg180-status.json](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg180-status.json)
  - output rotation: `deg180`
  - host present ownership: `direct_gbm`
  - last queued present source: `direct_gbm`
  - input event count: `33`
  - main app match hint: `ghostty`

- [deg270-status.json](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg270-status.json)
  - output rotation: `deg270`
  - host present ownership: `direct_gbm`
  - last queued present source: `direct_gbm`
  - input event count: `37`
  - main app match hint: `ghostty`

## File Metadata

- `deg0-status.json`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg0-status.json`
  - source path: `/tmp/surf-ace-deg0-status.json`
  - source capture time: `2026-04-12 08:28:36.783243919 +0000`
  - size: `18122` bytes
  - SHA-256: `777da3fc119c30a72139afce38e429ccd3010de1f4daf0b820f8b19dd4f09652`

- `deg90-status.json`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg90-status.json`
  - source path: `/tmp/surf-ace-deg90-status.json`
  - source capture time: `2026-04-12 08:28:50.294210814 +0000`
  - size: `18122` bytes
  - SHA-256: `071803f62cec0463ac5c5ea778a6db5e864f0ca3d95cc2202ec9fe0ecdfc793a`

- `deg180-status.json`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg180-status.json`
  - source path: `/tmp/surf-ace-deg180-status.json`
  - source capture time: `2026-04-12 08:29:03.732175899 +0000`
  - size: `18123` bytes
  - SHA-256: `9a03d0e78f14082a38e0201220cb16bca4dcc7dd78513ffdc0737ed2803d4cb0`

- `deg270-status.json`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-statuses/deg270-status.json`
  - source path: `/tmp/surf-ace-deg270-status.json`
  - source capture time: `2026-04-12 08:29:17.241138849 +0000`
  - size: `18123` bytes
  - SHA-256: `6fcc1699f1984aab551e6aaac6e7173085ca2b3cc507fc78e22a97fefa6a1557`
