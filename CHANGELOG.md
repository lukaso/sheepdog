# Changelog

Every release has an entry here. sheepdog follows semver; the stable interfaces are the subcommands
and the flags `sheepdog help` shows, the exit codes, the `--status-fd` and `--json` schemas (`"v": 1`, additive changes only)
and the `sheepdog:` prefix on its stderr messages (a usage error says what was wrong on that line,
then shows a `usage:` line).

## 0.1.0 (not yet released)

The first release.

- `sheepdog run`: runs a command and, when it ends, a limit fires (`--timeout`, `--max-mem`,
  `--max-procs`) or sheepdog gets TERM, kills every process it started, including processes that
  escaped with `setsid`, a double fork or reparenting. macOS (tracking by the kernel's responsible
  process) and Linux (a child subreaper), no root.
- `sheepdog kill <pid>` / `kill <job>`, `ps`, `strays [--kill]`, `sweep`, `doctor`.
- A journal of every job, so `sweep` can end what a killed sheepdog left behind.
- A usage error names what it rejected and the rule it broke; `run` without `--` runs nothing and
  prints the corrected command.
- macOS 12 or newer: `Sheepdog.app`, Developer ID-signed and notarized; Homebrew cask, `install.sh`,
  npm and pnpm. A job does not inherit the terminal's privacy permissions (Full Disk Access,
  Documents and so on); give Sheepdog Full Disk Access once, or run a job with
  `--inherit-terminal-permissions`.
- Linux: static binaries for `x86_64` and `aarch64`, and npm.
- Licensed under MIT OR Apache-2.0; every npm package carries both texts.
