# sheepdog

Run a command, and make sure every process it starts is gone at the end: also the ones that
escaped with `setsid`, a double fork, or reparenting to PID 1 or launchd. It works on macOS, on
Linux, and in a default Docker container, with no root, no cgroups and no systemd.

## Install

**macOS** (installs `Sheepdog.app`, signed and notarized, and a `sheepdog` command that runs it):

```sh
brew install --cask lukaso/tap/sheepdog
```

or, without Homebrew (into `~/Applications`, with the command in `~/.local/bin`; it tells you if
that is not on your PATH):

```sh
curl -fsSL https://github.com/lukaso/sheepdog/releases/latest/download/install.sh | sh
```

or with npm or pnpm (a global install; see [npm](#npm) below):

```sh
npm i -g @lukaso/sheepdog
```

The Homebrew cask needs Homebrew 5.1.11 or newer (from May 2026; `brew --version` shows yours). If
yours is older, run `brew update` first.

**Linux**, a static binary (`x86_64` and `aarch64`):

```sh
curl -fsSL https://github.com/lukaso/sheepdog/releases/latest/download/install.sh | sh
```

**Docker**, pinned and checked:

```dockerfile
ARG SHEEPDOG_VERSION=0.1.0
RUN curl -fsSL -o /usr/local/bin/sheepdog \
      "https://github.com/lukaso/sheepdog/releases/download/v${SHEEPDOG_VERSION}/sheepdog-linux-$(uname -m)" \
 && echo "<sha256 from the release's SHA256SUMS>  /usr/local/bin/sheepdog" | sha256sum -c - \
 && chmod +x /usr/local/bin/sheepdog
```

On Linux the checksum proves the download is intact, not where it came from: `SHA256SUMS` comes
from the same release. On macOS, install.sh checks the Developer ID signature before it installs,
which does prove where it came from. Homebrew checks the archive against the sha256 in the tap;
macOS then checks the signature and notarization when you first run it.

## Use

- `sheepdog run --timeout 5m -- npm test` stops the whole tree after 5 minutes.
- `sheepdog run --max-mem 2G -- python3 job.py` stops it if the tree uses more than 2 GB.
- `sheepdog strays` lists leaked processes of yours, biggest first.
- `sheepdog kill 4242` kills 4242 and the processes it provably started (see them first with
  `sheepdog ps 4242`).

To stop a running job, send TERM to sheepdog. Exit 124 means a limit fired. `sheepdog help
<command>` shows each command's options.

## For coding agents

Put this in your `CLAUDE.md` or `AGENTS.md`:

```markdown
## Commands that may hang or leak processes
Wrap them: `sheepdog run --timeout 5m -- <command>`. It kills the whole process tree when the command ends or a limit fires, including processes that escaped. Exit 124 means a limit fired; read the `sheepdog:` lines on stderr. To stop a job, send TERM to sheepdog (SIGINT to its pid does not reach the command). Leaked processes from earlier runs: `sheepdog strays`.
```

## Limits

sheepdog is not a sandbox. It does not reach:

- work started through another launcher: on macOS `open`, `osascript`, `xcodebuild test`, launchd
  agents; on Linux `systemd-run --user` and D-Bus activation;
- a process inside the job that disclaims itself again (Electron, VS Code and Chrome helpers), except
  on a best-effort basis;
- containers the job starts: they belong to `dockerd`;
- a process that leaves on purpose: `sudo`, another user, ptrace.

A caller that sends INT to sheepdog's pid alone and then SIGKILLs it (Node's `child.kill`,
`docker stop` with `STOPSIGNAL SIGINT`) kills only sheepdog; the rest of the tree is then ended by
your next `sheepdog run` or `sheepdog sweep` with the same `--owner` and the same state directory,
before a reboot (in a container: in the same container). A `run` with `--no-sweep` skips that, and
a `run` stops sweeping after 200 ms, so when many jobs were left, it can take more than one run. Send TERM instead.

On macOS a running job costs some CPU: sheepdog checks your processes four times a second to find
the ones that escaped. Measured on a busy Mac (about 1000 of your processes, load average 16): about 1.5% of one CPU
core per running job. The cost grows with the number of processes and with the Mac's load.

Where a delegated cgroup v2 is available, a cgroup is stronger: processes cannot leave it. sheepdog
is the portable floor, and on a Mac the only option.

## macOS privacy

A job under sheepdog does not inherit your terminal's privacy permissions (Full Disk Access,
Documents, Desktop and so on): sheepdog makes itself the job's responsible app, which is what lets
it track the whole tree. If a job needs a protected folder, give Sheepdog Full Disk Access once:
System Settings > Privacy & Security > Full Disk Access, click +, press Cmd-Shift-G, paste the path
of `Sheepdog.app` and press Return. It is in `~/Applications` for install.sh and for Homebrew with
`--appdir=~/Applications`, in `/Applications` for Homebrew otherwise, and inside the package for npm
and pnpm, where these commands print it:

- npm: `find "$(npm root -g)/@lukaso" -name Sheepdog.app -prune`
- pnpm: `find "$(pnpm root -g)/../.pnpm" -name Sheepdog.app -prune`

In a standard (not admin) account the row may not appear in the list after you add it; the grant
still works. The grant survives upgrades.
`sheepdog doctor --grants` checks whether Sheepdog can read `~/Documents` (this can show a macOS
privacy prompt).

Or run one job with `--inherit-terminal-permissions`: it keeps your terminal's permissions, and
sheepdog falls back to a weaker way of tracking the tree.

Uninstalling does not remove a grant. Remove it before you uninstall (afterwards macOS may no longer
find Sheepdog):

```sh
tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog
```

In an admin account the row then stays in the list, switched off; remove it with − if you like.

## npm

- Install globally (`npm i -g`, `pnpm add -g`). The `sheepdog` on your PATH then *is* sheepdog (a
  small `sh` launcher replaces itself with it), so it keeps your process id, signal settings and
  environment, and TERM to it reaches the job.
- Do not run it through `npx`, `pnpm dlx` or `bunx`: they start it as a child of their own, so TERM
  to the process you hold does not reach sheepdog.
- pnpm's own launcher adds a `NODE_PATH` to the job's environment, as it does for every global bin.
- If you use bun, check the installed files' permissions: a bun install has been measured to make
  them world-writable. Do not install with bun as root or into a shared prefix.

## Uninstall

First, if you gave Sheepdog Full Disk Access, remove the grant while it is still installed:

```sh
tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog
```

Then remove it the way you installed it:

- Homebrew: `brew uninstall --cask sheepdog` (with `--zap`, it also removes
  `~/.local/state/sheepdog`)
- install.sh on macOS: `rm -rf ~/Applications/Sheepdog.app ~/.local/bin/sheepdog`
- install.sh on Linux: `rm ~/.local/bin/sheepdog` (as root: `rm /usr/local/bin/sheepdog`)
- npm: `npm rm -g @lukaso/sheepdog`
- pnpm: `pnpm rm -g @lukaso/sheepdog`

pnpm 10 keeps a copy of the app in its store after `pnpm rm -g`. To remove it:

```sh
find "$(pnpm root -g)/../.pnpm" -maxdepth 1 \( -name '@lukaso+sheepdog@*' -o -name '@lukaso+sheepdog-darwin-universal@*' \) -exec rm -rf {} +
```

sheepdog keeps its job journals in `$SHEEPDOG_STATE` if set, else in `$XDG_STATE_HOME/sheepdog` if
that is an absolute path, else in `~/.local/state/sheepdog`; delete that directory to remove them. The privacy grant stays until you reset it (see
[macOS privacy](#macos-privacy)).

## Reference

- Exit codes follow `timeout(1)`: the command's own code; 124 when a limit fired (`--timeout`, a
  cap); 125 when sheepdog failed, for a usage error of `run`, or when a process was still alive at
  the kill deadline; a job ended by a TERM from outside dies of SIGTERM itself (143 in a shell).
  `--status-fd` says which, as one JSON line.
- Stable interfaces: the subcommands and flags, the exit codes, the `--status-fd` and `--json`
  schemas (`"v": 1`), and the `sheepdog:` prefix on its stderr messages (a usage error starts
  with `usage:` instead). Every release has a
  [CHANGELOG](CHANGELOG.md) entry.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT),
at your option. Unless you explicitly state otherwise, any contribution you submit for inclusion in
sheepdog, as defined in the Apache-2.0 license, is dual licensed as above, without any additional
terms or conditions.
