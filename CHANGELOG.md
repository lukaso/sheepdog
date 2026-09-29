# Changelog

Every release has an entry here (PLAN.md §10.8). sheepdog follows semver; the stable interfaces are
the subcommands and flags, the exit codes, the `--status-fd` and `--json` schemas (`"v": 1`,
additive changes only) and the `sheepdog:` prefix on stderr.

## 0.1.0 (not yet released)

The first release.

- `sheepdog run`: runs a command and, when it ends, a limit fires (`--timeout`, `--max-mem`,
  `--max-procs`) or sheepdog gets TERM, kills every process it started, including processes that
  escaped with `setsid`, a double fork or reparenting. macOS (tracking by the kernel's responsible
  process) and Linux (a child subreaper), no root.
- `sheepdog kill <pid>` / `kill <job>`, `ps`, `strays [--kill]`, `sweep`, `doctor`.
- A journal of every job, so `sweep` can end what a killed sheepdog left behind.
- macOS: `Sheepdog.app`, Developer ID-signed and notarized; Homebrew cask, `install.sh`, npm.
- Linux: static binaries for `x86_64` and `aarch64`.
