# Surf Ace Compositor Test Evidence

Date: 2026-04-12
Scope: preserved cargo test output for the current verified RACTER tty4 slice
Status: captured from the current canonical RACTER worktree; no merge performed, no deploy performed

## Evidence Set

Preserved test log directory:

- `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-test`

Captured command:

```bash
cd /home/clu/src/surf-ace-compositor
source ~/.cargo/env >/dev/null 2>&1
cargo test
```

Preserved raw output:

- [cargo-test.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-test/cargo-test.txt)

## What This Log Proves

This log is the exact rendered output from `cargo test` against the current dirty canonical RACTER worktree for the verified slice.

The preserved output shows:

- the crate built successfully in the `test` profile
- `src/lib.rs` unit tests passed: `60 passed; 0 failed`
- `src/main.rs` unit test target passed: `0 passed; 0 failed`
- `tests/host_failure_survivability.rs` passed: `1 passed; 0 failed`
- doc tests passed: `0 passed; 0 failed`

## File Metadata

- `cargo-test.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-test/cargo-test.txt`
  - line count: `86`
  - size: `6157` bytes
  - captured time: `2026-04-12 09:04:00.261569770 +0000`
  - SHA-256: `196d428f6ae68ec801b73041d388d0f2ad09875cff0bb92e977634b72bd2876b`
