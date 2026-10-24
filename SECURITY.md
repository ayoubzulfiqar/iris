# Security policy

Iris draws behind the desktop and loads wallpaper files the operator chooses.
Treat a bug that crosses that boundary as a vulnerability.

## Supported versions

| Version | Status |
| --- | --- |
| 0.1.x | Security fixes accepted on `main` |

Older snapshots are not maintained as separate release lines. Upgrade to the
current `main` commit that contains the fix.

## Report a vulnerability

Report privately. Do not open a public GitHub issue, pull request, or
discussion for a suspected vulnerability.

1. Open a draft advisory:
   <https://github.com/ayoubzulfiqar/iris/security/advisories/new>
2. Include the version or commit, the operating system, and a minimal way to
   reproduce the issue.
3. Say what an attacker gains: code execution, file read outside the wallpaper
   root, privilege change, or a daemon crash that survives bad input.

If private vulnerability reporting is not enabled on the repository yet, contact
the maintainer [@ayoubzulfiqar](https://github.com/ayoubzulfiqar) directly and
wait for a private channel. Still do not describe the issue in public.

The maintainer aims to acknowledge a report within 7 days and to confirm or
reject the issue within 30 days. A fix and an advisory follow when the report
is valid. Dates slip when reproduction depends on a compositor or a GPU that
the maintainer does not have.

## In scope

- Path traversal or symlink escape in `iris://` or package lookup that reads
  outside the wallpaper root.
- IPC parsing that accepts an oversized frame, a malformed request that panics
  the daemon, or a call that runs with another user's socket or pipe.
- Loading a wallpaper that executes code outside the webview sandbox, other
  than the web content the operator chose to display.
- A crash or memory-safety bug in a runtime path (`unwrap` on a library path,
  unsound `unsafe`, use-after-free around a window or FFmpeg buffer).
- The manager or daemon retaining a secret from the environment and writing it
  to a log.

## Out of scope

- Wallpaper content that is already allowed to run: a web wallpaper's own
  JavaScript, WebGL, or network requests inside the webview.
- A compositor or shell that ignores layer-shell or WorkerW placement. That is
  a desktop-integration limit, documented in
  [platforms](docs/platforms.md).
- Missing hardware decode. Video falls back to a software decoder.
- Denial of service by a huge image the operator asked the daemon to open,
  inside the documented size caps.
- Vulnerabilities in unpacked third-party crates that Iris does not expose.
  Report those upstream, and mention Iris only when a caller in this tree
  makes the bug reachable.

## Disclosure

Valid reports are fixed on `main` first. The advisory is published after the
fix is available, unless the reporter and the maintainer agree on a shorter
window. Credit is given when the reporter wants it.
