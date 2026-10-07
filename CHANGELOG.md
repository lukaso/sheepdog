# Changelog

Every release has an entry here. sheepr follows semver; the stable interfaces are the subcommands
and the flags `sheepr help <command>` shows, the exit codes, the `--status-fd` and `--json`
schemas (`"v": 1`, additive changes only) and the `sheepr:` at the start of the first line of
each message sheepr itself writes to stderr (for a usage error, that line says what was wrong;
`sheepr` alone prints the overview and exits 2).

## Unreleased

- `run --status-fd`: the status line waits at most 1 s for its reader. A reader that does not
  read it, or that has closed its end, can no longer keep sheepr running after the job is killed:
  sheepr says on stderr that the line did not arrive (also with `--quiet`) and exits with its
  usual code ([#14](https://github.com/lukaso/sheepr/issues/14)).
- `sweep`: a journal it skips because it cannot read it, or will not act on it for safety, is said
  on stderr, one line that names the file; the auto-sweep before each `run` puts the same skip in
  the `--status-fd` notes. Before, only a debug build saw it. A job kept with `--leave-strays` is
  still skipped without a word, and the exit code does not change
  ([#15](https://github.com/lukaso/sheepr/issues/15)).

## 0.1.0 (2026-10-06)

The first release.

- `sheepr run`: runs a command and, when it ends, a limit fires (`--timeout`, `--max-mem`,
  `--max-procs`) or sheepr gets TERM, kills every process it started, including processes that
  escaped with `setsid`, a double fork or reparenting. macOS (tracking by the kernel's responsible
  process) and Linux (a child subreaper), no root.
- `sheepr kill <pid>` / `kill <job>`, `ps`, `strays [--kill]`, `sweep`, `doctor`.
- A journal of every job, so `sweep` can end what a killed sheepr left behind.
- A usage error names what it rejected and the rule it broke; `run` without `--` runs nothing and
  prints the corrected command.
- macOS 12 or newer: `Sheepr.app`, Developer ID-signed and notarized; Homebrew cask, `install.sh`,
  npm and pnpm. A job does not inherit the terminal's privacy permissions (Full Disk Access,
  Documents and so on); give Sheepr Full Disk Access once, or run a job with
  `--inherit-terminal-permissions`.
- Linux: static binaries for `x86_64` and `aarch64`, and npm.
- Licensed under MIT OR Apache-2.0; every npm package carries both texts.
