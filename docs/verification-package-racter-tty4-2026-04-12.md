# Surf Ace Compositor Verification Package

Date: 2026-04-12
Scope: current verified dirty slice only
Status: no merge performed, no deploy performed

## Verified on RACTER tty4

- Canonical repo: `/home/clu/src/surf-ace-compositor`
- Live host path uses the standalone compositor binary, not Electron
- Verified socket for the working tty4 path: `/tmp/surf-ace-zsh-tty4.sock`
- Verified main app for the live demo: Ghostty running interactive `zsh`
- Verified present path: `direct_gbm`
- `SURF_ACE_HOST_RUNTIME_FORCE_READBACK` remains inactive for the verified path
- `capture_screen` now exports the panel-equivalent view for all verified rotations
- Real-path rotation and capture are verified for `deg0`, `deg90`, `deg180`, and `deg270`
- Typed input reaches the shell on the live tty4 demo

## Operator Workflow Entry Points

Repo-local operator entry points:

- [README.md](/home/clu/src/surf-ace-compositor/README.md): run paths and operator quick path
- `surf-ace-compositor --help`: verified top-level tty4 workflow example
- `surf-ace-compositor serve --help`: verified host runtime launch example
- `surf-ace-compositor ctl --help`: verified `get_status`, `set_output_rotation`, and `capture_screen` examples

Verified host/runtime entry point:

```bash
cd /home/clu/src/surf-ace-compositor
source ~/.cargo/env >/dev/null 2>&1
./target/debug/surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock
```

Verified control entry points:

```bash
./target/debug/surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{"type":"set_output_rotation","rotation":"deg90"}'
./target/debug/surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{"type":"capture_screen","output_path":"/tmp/surf-ace-capture.png"}'
./target/debug/surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{"type":"get_status"}'
```

Supporting canonical spec:

- [surf-ace-compositor-v1-spec.md](/Users/mike/shared-workspace/surf-ace-compositor/surf-ace-compositor-v1-spec.md)
- [verification-package-racter-tty4-2026-04-12.patch](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12.patch): preserved patch for the current compositor code delta behind this verified slice
- [verification-package-racter-tty4-2026-04-12-capture-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-capture-evidence.md): preserved rotation-capture evidence for the verified tty4 run
- [verification-package-racter-tty4-2026-04-12-status-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-status-evidence.md): preserved per-rotation runtime-status evidence for the verified tty4 run
- [verification-package-racter-tty4-2026-04-12-help-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help-evidence.md): preserved rendered CLI help for the verified operator workflow
- [verification-package-racter-tty4-2026-04-12-test-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-test-evidence.md): preserved cargo test output for the current verified slice
- [verification-package-racter-tty4-2026-04-12-worktree-state-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-worktree-state-evidence.md): preserved raw worktree-state output for the current verified slice
- [verification-package-racter-tty4-2026-04-12-repo-identity-evidence.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-repo-identity-evidence.md): preserved HEAD and branch identity for the exact checkout behind this package
- [verification-package-racter-tty4-2026-04-12-artifact-inventory.md](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-artifact-inventory.md): consolidated path-and-checksum inventory for the preserved verification artifacts

## Patch Verification

Patch artifact facts:

- Path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12.patch`
- SHA-256: `7fb5ea55d45037fdcdbf24d9fd6c280ba2ae7f2b636cdbce9a2a275c47ef0fd9`
- Line count: `1791`

Scope note:

- the patch artifact preserves the verified compositor code delta
- this dated verification package is preserved separately as a sibling artifact and is intentionally excluded from the patch payload so the hash/check can stay stable

To regenerate and verify the patch against the live RACTER worktree:

```bash
cd /home/clu/src/surf-ace-compositor
verify=/tmp/surf-ace-compositor-verify-tty4-2026-04-12.patch

git diff --binary -- Cargo.lock Cargo.toml src/control.rs src/lib.rs src/main.rs src/runtime.rs > "$verify"
git diff --binary --no-index -- /dev/null src/screen_capture.rs >> "$verify" || test "$?" -eq 1

cmp -s docs/verification-package-racter-tty4-2026-04-12.patch "$verify"
wc -l docs/verification-package-racter-tty4-2026-04-12.patch
sha256sum docs/verification-package-racter-tty4-2026-04-12.patch
rm -f "$verify"
```

Expected verification result:

- `cmp -s` exits `0`
- `wc -l` reports `1791`
- `sha256sum` reports `7fb5ea55d45037fdcdbf24d9fd6c280ba2ae7f2b636cdbce9a2a275c47ef0fd9`

## Dirty Files

Current dirty worktree files after packaging this note:

- `M Cargo.lock`
- `M Cargo.toml`
- `M src/control.rs`
- `M src/lib.rs`
- `M src/main.rs`
- `M src/runtime.rs`
- `?? docs/verification-package-racter-tty4-2026-04-12.patch`
- `?? docs/verification-package-racter-tty4-2026-04-12-capture-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-captures/deg0.png`
- `?? docs/verification-package-racter-tty4-2026-04-12-captures/deg90.png`
- `?? docs/verification-package-racter-tty4-2026-04-12-captures/deg180.png`
- `?? docs/verification-package-racter-tty4-2026-04-12-captures/deg270.png`
- `?? docs/verification-package-racter-tty4-2026-04-12-status-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-statuses/deg0-status.json`
- `?? docs/verification-package-racter-tty4-2026-04-12-statuses/deg90-status.json`
- `?? docs/verification-package-racter-tty4-2026-04-12-statuses/deg180-status.json`
- `?? docs/verification-package-racter-tty4-2026-04-12-statuses/deg270-status.json`
- `?? docs/verification-package-racter-tty4-2026-04-12-help-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-help/top-level-help.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12-help/serve-help.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12-help/ctl-help.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12-test-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-test/cargo-test.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12-repo-identity-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-repo-identity/repo-identity.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12-artifact-inventory.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-worktree-state-evidence.md`
- `?? docs/verification-package-racter-tty4-2026-04-12-worktree-state/git-status-short.txt`
- `?? docs/verification-package-racter-tty4-2026-04-12.md`
- `?? src/screen_capture.rs`

## Merge/Cleanup Decision Context

This slice is intentionally still dirty. The working compositor path, operator workflow, spec workflow note, and binary help are all in place, but Flynn still needs to decide when to:

- merge the verified compositor/runtime/help changes
- keep or delete this dated verification-package artifact
- do any later cleanup beyond the verified slice
