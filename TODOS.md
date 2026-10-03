# TODOS

These become GitHub issues when the repo is first pushed (operator, 2026-09-25).

## A drop-in `treeKill()` with npm tree-kill's exact API

- **What:** `require('@lukaso/sheepdog/tree-kill')`, with tree-kill's exact signature `treeKill(pid, signal?, callback?)`. It is implemented by calling `sheepdog kill <pid> --json`. The CLI stays the main product; this is a compatibility entry point for Node code.
- **Why:** tree-kill (18.4 M downloads a week) kills only the live ppid tree (`pgrep -P` / `ps --ppid`), so escapees survive. Swapping one `require` line would make those kills reach escapees whose parent is alive, and on macOS `setsid` escapes too.
- **Limit:** it is the no-setup mode. The full guarantee still needs `sheepdog run` at launch.
- **Pros:** the largest adoption lever the DevEx review found; no new CLI surface.
- **Cons:** one more API to version; it is only as good as `kill <pid>`.
- **Depends on:** phase 2 (`kill <pid> --json`).
- **Not planned:** making the whole API tree-kill-compatible. Its API is a Node function, and most users run commands (PLAN.md §10.12).

## A Claude Code hook that wraps agent commands in `sheepdog run`

- **What:** an optional PreToolUse hook, shipped as an example, that rewrites a Bash tool call such as `python3 -m unittest …` into `sheepdog run --timeout <default> -- …`.
- **Why:** every leak in PLAN.md §1 came from an agent-launched command. A CLAUDE.md rule depends on the agent following it; a hook does not.
- **Cons:** rewriting commands is invasive (quoting, pipelines, background `&`, interactive commands). liveapp's docs/enforcement.md shows that command-string matchers are hard to make both complete and safe. It is also a new integration channel.
- **Needs:** its own design review before any build.
- **Depends on:** v1 shipped, and the agent first-use eval (PLAN.md §10.9) passing.

## The macOS scan's cost on a machine with many processes (the first item after v0.1.0)

- **What:** on each 250 ms tick the macOS scan lists every process and reads `proc_pidinfo` (BSD info) for each, of every user, to find its uid; for each same-uid process it then reads `proc_pidinfo` again (the unique ids) and the responsibility SPI. Measured 2026-09-25 on a Mac with 800 processes: about 15 ms of CPU per 3 s (0.5 %) in the debug build. The cost grows with the process count, so it may pass 1 % at about 1600 processes.
- **Why not now:** a per-identity cache cut it to about 0.25 %, but it depends on facts not changing for a process. Responsibility changes to self on an exec with disclaim (cell 24 went red when members were cached), and whether a cached non-member's facts can change in a way that makes it a member is not tested. It was removed.
- **Measured on a busy machine (2026-10-01):** about 960 of the operator's processes, load average 16: 1.34 % of one core in the release build, 1.86 % in the debug build. v0.1.0 ships with the cost stated in the README; the scan cell measures the cost against one tick's worth of the same system calls made by the test at the same time (macOS), so load and process count cancel out; see PHASE3.md, the scan-cost decision.
- **With any change to the scan:** update the scan cell's reference (tests/s2.rs `one_tick_of_scan_calls`), which repeats the scan's calls by hand; otherwise the cell keeps allowing up to SCAN_K (1.8) times the old calls, measured against the old reference.
- **Options:** Then: a longer tick when nothing new appears; a cache of non-members only, with a cell that proves a cached non-member cannot become a member; or a kqueue NOTE_FORK watch on the members.

## Dead zombie check in the root-disclaim control mode

- **What:** `responsible_to` in src/macos.rs filters `pbi_status != SZOMB`, but `proc_pidinfo(PROC_PIDTBSDINFO)` fails for a zombie (measured 2026-09-25), so the check never fires. The failure already filters zombies. Remove it with the next change to that mode.

## What v0.1.0 did not measure

- **What:** each of these is stated in the README or PHASE3.md as not measured. Measure it, then change the text.
  - The Homebrew cask into `/Applications` (Homebrew's default): the Full Disk Access grant, and `tccutil reset`. Only `--appdir=~/Applications` was measured.
  - `tccutil reset` after an uninstall. The docs say to reset before uninstalling, because PLAN.md §4.4 saw `-10814` when the bundle was gone.
  - Intel Macs. The x86_64 slice is checked only statically (`minos`, the exec door); the operator's Mac has no Rosetta, so it never ran.
  - macOS 12 to 26. Only macOS 27 on Apple silicon was run.
  - A job whose working directory is a protected folder (Documents, Desktop, Downloads, iCloud Drive), on a release build with no Full Disk Access: does the first access show a prompt, wait until `--timeout`, or fail at once? If it can wait, the timeout message should name privacy as a possible cause.
  - The grant after an upgrade by npm, pnpm or Homebrew. Only `install.sh` over `install.sh` was measured. pnpm's path changes with each version.
  - pnpm and bun installs on Linux. npm is tested there (Alpine and Debian images, the dist-linux leg); pnpm and bun only on macOS.
  - The Linux PATH hint in a real Linux terminal window (zsh `~/.zshrc`, bash `~/.bashrc`). The premise rows run macOS's own zsh and bash.

## `sheepdog help <command>` explains each flag (v0.1.1)

- **What:** today `help <command>` prints the usage line only. Add what each flag does and its default, and the SIZE grammar (K, M, G; binary) and the DURATION grammar (ms, s, m, h, d; a bare number is seconds). Usage errors already name the rule they broke (v0.1.0).

## `strays` text output

- **What:** add a header row; show ages as `3m` or `2h`, not raw seconds; show the ID that `--pid PID:ID` needs (today only `--json` prints it); plain words for what `puniq 1246 (dead)` means.

## `ps` refusal wording

- **What:** `sheepdog ps 1` says "refusing to kill pid 1 … Nothing was signalled", although `ps` only reads. In `ps`, say that `kill` would refuse it, and why.

## `doctor --grants` checks a folder that Full Disk Access alone opens

- **What:** it reads `~/Documents`. If the user allows Documents at the macOS prompt, the check passes while Full Disk Access is still off. Read a location that only Full Disk Access opens (for example `~/Library/Safari`), and say which grant it proves.
