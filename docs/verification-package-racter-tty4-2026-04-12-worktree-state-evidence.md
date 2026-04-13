# Surf Ace Compositor Worktree State Evidence

Date: 2026-04-12
Scope: preserved raw dirty-worktree state for the current verified RACTER tty4 slice
Status: captured from the current canonical RACTER worktree; no merge performed, no deploy performed

## Evidence Set

Preserved worktree-state directory:

- `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-worktree-state`

Captured command:

```bash
cd /home/clu/src/surf-ace-compositor
git status --short --untracked-files=all -- \
  Cargo.lock \
  Cargo.toml \
  src/control.rs \
  src/lib.rs \
  src/main.rs \
  src/runtime.rs \
  src/screen_capture.rs \
  docs/verification-package-racter-tty4-2026-04-12.patch \
  docs/verification-package-racter-tty4-2026-04-12.md \
  docs/verification-package-racter-tty4-2026-04-12-capture-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-captures/deg0.png \
  docs/verification-package-racter-tty4-2026-04-12-captures/deg90.png \
  docs/verification-package-racter-tty4-2026-04-12-captures/deg180.png \
  docs/verification-package-racter-tty4-2026-04-12-captures/deg270.png \
  docs/verification-package-racter-tty4-2026-04-12-status-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-statuses/deg0-status.json \
  docs/verification-package-racter-tty4-2026-04-12-statuses/deg90-status.json \
  docs/verification-package-racter-tty4-2026-04-12-statuses/deg180-status.json \
  docs/verification-package-racter-tty4-2026-04-12-statuses/deg270-status.json \
  docs/verification-package-racter-tty4-2026-04-12-help-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-help/top-level-help.txt \
  docs/verification-package-racter-tty4-2026-04-12-help/serve-help.txt \
  docs/verification-package-racter-tty4-2026-04-12-help/ctl-help.txt \
  docs/verification-package-racter-tty4-2026-04-12-test-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-test/cargo-test.txt \
  docs/verification-package-racter-tty4-2026-04-12-repo-identity-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-repo-identity/repo-identity.txt \
  docs/verification-package-racter-tty4-2026-04-12-artifact-inventory.md \
  docs/verification-package-racter-tty4-2026-04-12-worktree-state-evidence.md \
  docs/verification-package-racter-tty4-2026-04-12-worktree-state/git-status-short.txt
```

Preserved raw output:

- [git-status-short.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-worktree-state/git-status-short.txt)

## What This Output Proves

This file is the exact rendered `git status --short --untracked-files=all` output for the verified slice artifacts and compositor code paths in the canonical RACTER worktree.

It preserves the raw modified vs untracked shape behind the dated verification package without relying on later summary lists.

## File Metadata

- `git-status-short.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-worktree-state/git-status-short.txt`
  - line count: `30`
  - size: `1807` bytes
  - captured time: `2026-04-12 10:18:53.345577804 +0000`
  - SHA-256: `4336185fd11e93b6045757784322d4ef21cd4ba14f4280565f05c37a959e59b8`
