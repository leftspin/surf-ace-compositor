# Surf Ace Compositor Repo Identity Evidence

Date: 2026-04-12
Scope: preserved repo identity anchor for the current verified RACTER tty4 slice
Status: captured from the current canonical RACTER checkout; no merge performed, no deploy performed

## Evidence Set

Preserved repo-identity directory:

- `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-repo-identity`

Captured command sequence:

```bash
cd /home/clu/src/surf-ace-compositor
printf "repo_path=%s\n" "$PWD"
printf "show_toplevel=%s\n" "$(git rev-parse --show-toplevel)"
printf "head_sha=%s\n" "$(git rev-parse HEAD)"
printf "branch_name=%s\n" "$(git branch --show-current)"
printf "symbolic_ref=%s\n" "$(git symbolic-ref --quiet --short HEAD || printf detached)"
printf "origin_url=%s\n" "$(git remote get-url origin)"
printf "head_subject=%s\n" "$(git show -s --format=%s HEAD)"
```

Preserved raw output:

- [repo-identity.txt](/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-repo-identity/repo-identity.txt)

## What This Output Proves

This file preserves the minimal identity facts needed to anchor the verified slice to the exact checkout that produced the package artifacts:

- canonical repo path and git toplevel
- current `HEAD` commit SHA
- current branch context
- `origin` remote URL
- current `HEAD` subject line

## File Metadata

- `repo-identity.txt`
  - preserved path: `/home/clu/src/surf-ace-compositor/docs/verification-package-racter-tty4-2026-04-12-repo-identity/repo-identity.txt`
  - line count: `7`
  - size: `291` bytes
  - captured time: `2026-04-12 10:04:45.109058358 +0000`
  - SHA-256: `560ad558d625d6eb6a52fb5cd9cc78efc6a67e962fec65d0f54631d8461b6656`
