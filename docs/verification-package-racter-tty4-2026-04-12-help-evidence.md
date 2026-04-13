# Surf Ace Compositor Help Evidence

Date: 2026-04-12
Scope: preserved rendered CLI help output for the verified RACTER tty4 workflow
Status: captured from the current verified RACTER binary; no merge performed, no deploy performed

## Evidence Set

Preserved help directory:

- `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help`

These text files are preserved renders of the current verified binary help surfaces from:

- `/home/clu/src/surf-ace-compositor/target/debug/surf-ace-compositor --help`
- `/home/clu/src/surf-ace-compositor/target/debug/surf-ace-compositor serve --help`
- `/home/clu/src/surf-ace-compositor/target/debug/surf-ace-compositor ctl --help`

## What Each Output Proves

- [top-level-help.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/top-level-help.txt)
  proves the operator-facing top-level binary help exposes the `serve` and `ctl` subcommands and includes the verified tty4 workflow examples for host launch, rotation control, and `capture_screen`.
- [serve-help.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/serve-help.txt)
  proves the `serve` subcommand help exposes `--runtime`, `--socket-path`, and the verified host launch example used on RACTER tty4.
- [ctl-help.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/ctl-help.txt)
  proves the `ctl` subcommand help exposes `--socket-path`, `--request-json`, and the verified control examples for `get_status`, `set_output_rotation`, and `capture_screen`.

## File Metadata

- `top-level-help.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/top-level-help.txt`
  - line count: `16`
  - size: `766` bytes
  - captured time: `2026-04-12 09:00:15.987251485 +0000`
  - SHA-256: `a80213aecce81860f62a5bc2edceca69c4a2b69a6a19526cf7beddb473965aed`

- `serve-help.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/serve-help.txt`
  - line count: `11`
  - size: `429` bytes
  - captured time: `2026-04-12 09:00:15.990251477 +0000`
  - SHA-256: `06749638f5d906a72661a3c3fa510196cb6ea471945f1c66a9dc97b9c5388bc6`

- `ctl-help.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-help/ctl-help.txt`
  - line count: `13`
  - size: `788` bytes
  - captured time: `2026-04-12 09:00:15.992251472 +0000`
  - SHA-256: `49217436a3656e0d3c979ecd6afe39f8665f4da7bc4d1e504fb6433c3e347999`
