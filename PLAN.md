# sheepdog — plan

**Status:** r12, 2026-09-25. Phase 0 gate PASSED on macOS and Linux (§7.1). All §8 questions answered. Renamed to sheepdog; DevEx review done (§10); §3.0 no-setup modes and the DevEx additions are not yet adversarially reviewed. Review rounds 1–6 done (7 P1s in rounds 1–3, **none in rounds 4–6**; all P1s re-measured by the planner and folded in; see §10). Nothing built.

**One sentence:** a small static binary that runs a command and, when the command ends, times out, uses too much memory, or you ask, kills **every process the command started**, including those that called `setsid`, double-forked, or were reparented to PID 1 or launchd. It works on macOS, on Linux, and inside a default Docker container, with no root, no cgroups and no systemd.

```sh
# With a supervisor (the guarantee):
sheepdog run --timeout 30s --max-mem 2G -- python3 -m unittest …   # the whole tree dies at 30 s or 2 GB
sheepdog run -- pnpm exec vitest run                                # strays are reaped even on a clean exit

# With no setup (best effort, labelled evidence):
sheepdog kill <pid>      # kill the PROVED tree of any process; list the suspects
sheepdog strays          # your processes that were reparented to PID 1 / launchd, biggest first

# Around both:
sheepdog ps   <job|pid>  # the tree, and the evidence for each process (kills nothing)
sheepdog kill <job>      # kill a running supervised job from outside
sheepdog sweep --owner liveapp   # kill the trees of THIS owner's jobs whose supervisor died
sheepdog doctor          # which mechanisms work on this machine, and what is degraded
```

---

## 1. Why this, and why now

The same failure happened again and again:

| When | What escaped | Cost |
|---|---|---|
| 2026-09-07, EOS | nx and jest workers reparented to docker-init, up to 2.5 days old | `pids.max` 512/512, the loop was blind for 2 h 40 min |
| 2026-09-07, Mac | `script` pty runners in their own session: 351 `script` + 751 `bash` | WindowServer watchdog, hard reset |
| gate, #332 | `bash -c 'set -m; "$@"'` execs the last command, so no group is made | the gate hung for 1800 s |
| 2026-09-25, Mac | a ccwho mutant: `kill $P` hit the subshell, the orphaned `python3` looped | 67 GB, kernel panic, several forced restarts |

liveapp tried five approaches: process groups, pid ledgers, bash watchdogs, the systemd scope, and cgroupfs (plus the Docker "box", which was never wired in). Only cgroupfs is reliable, and it works only on Linux with a delegated cgroup subtree. **macOS has no working containment.** The repo's own lesson: *"Membership has to be something children cannot leave."*

## 2. The platform facts this plan stands on (measured on 2026-09-25)

All probes are in `spike/`. Rounds 1 and 2 of the review added probes. `†` marks results the planner re-ran before accepting them.

| Fact | Result |
|---|---|
| macOS `kqueue` `NOTE_TRACK` | not supported (`ENOTSUP`) † |
| macOS env of Apple platform binaries | hidden (`/bin/bash`, `/bin/sleep`) † |
| macOS `p_puniqueid` (original parent's unique id) | readable without root; survives `setsid` and orphaning † and `exec` while the parent lives. **Resets to 1 (launchd) when a process execs after it was reparented.** It is not answered for zombies. As the only evidence it loses **114–117 of 300** grandchildren in a fast double-fork †. It is a weak, second fact. |
| macOS **responsibility** (exported private SPI: `responsibility_spawnattrs_setdisclaim`, `responsibility_get_uniqueid_responsible_for_pid`) | copied at fork, so race-free † while the responsible process lives. Kept across fork, `posix_spawn`, `setsid`, double-fork and exec. |
| … **but when the responsible process exits**, even as an unreaped zombie | **every descendant becomes responsible for itself** † |
| … **so the supervisor makes ITSELF responsible**: at startup it re-execs itself with `POSIX_SPAWN_SETEXEC` + disclaim (same pid), then spawns the root **without** disclaim | after the root is reaped, a `setsid` double-fork stray still reports the supervisor † |
| … **disclaim removes the terminal's privacy (TCC) grants** | iTerm has Full Disk Access: `ls ~/Library/Mail` exits 0 without disclaim, and gives `Operation not permitted` with disclaim † |
| responsibility for other users' processes and for zombies | not answered (−1) |
| `open -g` of an app, a `launchctl submit` job | responsible for themselves: not contained (§4.1) |
| `p_uniqueid` | a counter from boot: **repeats after a reboot**. So does Linux (pid, start time). |
| Linux `PR_SET_CHILD_SUBREAPER`, unprivileged, default Docker | works, survives `exec` †. **The supervisor must reap adopted orphans**, or they stay zombies. |
| Linux `pidfd_open` / `pidfd_send_signal`, default Docker (musl and glibc) | work |
| Linux: create a cgroup in a default Docker container | `EROFS` † |

**Consequence:**
- **Linux:** a subreaper supervisor sees every descendant while it lives.
- **macOS:** a supervisor that is itself the *responsible* process gives membership that a child cannot leave by `setsid`, double-fork or its parent's exit, **for as long as the supervisor lives.** The cost is the terminal's TCC grants (§4.4).

## 3. Design

### 3.0 Modes: supervised and no-setup

The evidence that links an escaped process to its tree **disappears when the processes in between exit** (§2). So what sheepdog can prove depends on when it first looks.

| Mode | Setup | What it guarantees |
|---|---|---|
| `run` (§3.1–3.5) | prefix the command | every member, including escapees, while the supervisor lives |
| `kill <pid>`, `strays` | none | the **proved** part of a tree, plus a **labelled** list of suspects. Never a guarantee. |

The no-setup modes use the same scan, the same identity checks and the same freeze → verify → kill → repeat loop with a deadline (§3.3). They differ only in where the membership comes from.

#### `sheepdog kill <pid>`

**Target checks:** same uid; not PID 1; not sheepdog itself, its ancestors or the caller's shell. The target's identity (uniqueid on macOS; start time on Linux) is captured at the call and re-verified before every signal.

**A kill never reaches wider than the named target (DevEx D14, from the outside voice).** If the target is a **supervisor**, its subtree is its job, so the job's own kill runs (§3.3). If the target is a **member** of a running supervised job, only that member and its own subtree are killed, and the output adds: `pid 4242 belongs to running job 7f3a; to end that job: sheepdog kill 7f3a`. `--dry-run` shows exactly the set that would be killed.

**Proved members** (killed):
- live descendants by the ppid chain, iterated to a fixed point;
- **macOS:** plus live processes whose `p_puniqueid` is in the known set (the target and every proved member). This proves a `setsid` escapee whose parent is alive, or was alive during the scan.

**Suspects** (listed, **not** killed unless `--include-suspects`). Each needs **all** of: same uid, orphaned (ppid is PID 1, launchd, or a subreaper), started **after** the target, and one link:
- the same session or process group as the target or a proved member;
- **macOS:** a `p_puniqueid` that is a dead id greater than the target's uniqueid **and** the same responsible process as the target (round 7: without the second condition, any app helper born after the target matched).

Each suspect row shows which link matched. Siblings and `| tee` partners can match (round-1 F3), so suspects stay opt-in.

**A supervisor inside the kill set is ended first (round 7).** If the proved set contains a `sheepdog run` supervisor (for example `kill <npm pid>` while npm runs `sheepdog run -- vitest`), sheepdog sends it TERM and waits up to its kill deadline, so that the inner job's own kill reaches its escapees. Only then does the freeze loop (§3.3) run on the rest.

**Pid reuse:** `kill` accepts `pid:id` (the `id` from `--json` or `strays --json`), and refuses when the identity no longer matches. `strays --kill` verifies the identity captured when it listed, not a new one after the tty prompt.

**Output:** a table of proved members and suspects, with evidence, memory and age. After a kill it always ends with the count of suspects left alive, for example `sheepdog: killed 5 proved processes; 2 suspects left alive (--include-suspects to kill them).` (outside voice F6). `--dry-run` = `sheepdog ps <pid>`. `--json` gives one object per process: `{pid, id, class: "proved"|"suspect", evidence: [...], mem, age, cmd}`. **Exit codes (round 7):** 0 = every targeted process is gone, **including "the target was already gone"** (the goal is met; the output says so); 1 = **refused** (nothing was signalled; the target is alive); 125 = the kill deadline passed with members alive; 2 = usage error (as for every subcommand except `run`, §10.5). **Job ids always start with `j-`** (`j-7f3a`), so a bare number is always a pid.

#### `sheepdog strays`

It **lists**; it kills nothing unless asked.

**What counts as an orphan:**
- **macOS:** same uid, ppid is launchd, **`p_puniqueid` ≠ 1** (its original parent was not launchd, so it was reparented), **and its responsible process is not a live app whose bundle contains the process's executable.** Measured in round 7: without the last rule, 8 processes on the operator's Mac matched, all app helpers that double-fork on purpose (the crashpad handlers of Chrome, VS Code, Claude, ChatGPT, Discord and Google Drive, and Spotify Helper). Their responsible process is their own live app. The 2026-09-25 leak's responsible process was the terminal, so it is still listed. `open -g` and `launchctl` jobs have `puniq` 1 (round 2). **Stated limit:** a daemon that execs after it was reparented also resets to `puniq` 1 (round 2), so it looks launchd-started and is not listed.
- **Linux (name-based heuristic, stated):** same uid, and the ppid is PID 1 or a process whose name is on a known-reaper list (`systemd`, `tini`, `docker-init`, `dumb-init`, `catatonit`, `s6-svscan`). Round 7 measured that nothing in `/proc` shows whether a process set the subreaper flag, and that a unit's `MainPID` needs systemd's D-Bus. **In a container whose PID 1 is not an init** (for example `sh`), a direct child of PID 1 and a real orphan look the same (both ppid 1). They are listed, but marked `(child of PID 1 app?)`, and `--kill` skips them unless named. `docker exec` processes have ppid 0 and are not listed. Each row shows why it matched.

**Columns, biggest memory first:** pid, memory (`phys_footprint` / RSS), age, CPU, command, and the **best-known origin**:
- the session or group leader, if it is alive (for example "pgid 90653 = Claude Code Bash tool shell");
- the tty;
- the cwd;
- macOS: the dead `puniq`, and whether an alive process shares it.

**Filters:** `--min-mem 1G`, `--older-than 10m`, `--cmd <regex>`, `--json`.

**Killing:** `sheepdog strays --kill` needs at least one filter and prints the list first. Without `--yes` it asks on a tty and refuses in a non-tty. Each orphan is killed as `sheepdog kill <pid>` would kill it (its proved subtree, never wider; D14). Rows that belong to a **running** supervised job are marked `(job j-7f3a, running)` and are skipped by `--kill` unless named with `--pid <pid:id>` (a filter). `--min-mem 0` or `--older-than 0` do not count as a filter.

**Today's crash:** `sheepdog strays --min-mem 1G` would have listed `python3 … 65 GB, age 1 h, origin: pgid = Claude Code Bash tool shell, cwd ~/projects/ccwho-asks`.

### 3.1 Shape

- **macOS:** `sheepdog run` first re-execs itself with `SETEXEC` + disclaim. Measured (round 3): this keeps the pid, uniqueid, ppid, pgid, sid, foreground-tty status and non-CLOEXEC fds, and the caller's `waitpid` still gets the exit code.
  - **Already re-exec'd?** It is if `responsible_uniqueid(self) == uniqueid(self)`. There is at most one attempt: the re-exec carries `SHEEPDOG_REEXEC_PID=<own pid>`, and if that mark is present but the check still fails, sheepdog falls back instead of re-exec'ing again. The mark is removed before the root starts (measured: it never reaches the job), so a nested sheepdog still disclaims.
  - **Re-exec path:** the kernel's own (`proc_pidpath`), never `argv[0]`, which may be a bare `sheepdog` from a PATH lookup.
  - It then `posix_spawn`s the root **without** disclaim.
- **Linux:** it sets the subreaper flag, then forks and execs the root.
- **The root gets the caller's signal state:** the dispositions and mask sheepdog was started with. sheepdog changes none of them itself (its own C `main`, so Rust's runtime does not ignore SIGPIPE), and it spawns the root without `SETSIGDEF`/`SETSIGMASK` (cell 23).
  - **One exception, SIGCHLD:** sheepdog sets SIGCHLD to default before anything else, because a supervisor whose children are reaped automatically cannot wait for them (a caller that ignores SIGCHLD made the Linux supervisor wait out the escapees and never kill: review round 2, P1-B). `posix_spawn` cannot give the child an ignored SIGCHLD while the parent has the default, so **the root gets SIGCHLD at default**. **Consequence:** a job that relied on an inherited ignored SIGCHLD (so that it never has to wait for its children) collects zombies until it exits.
  - **Linux, children sheepdog did not create:** a shell that runs `job & exec sheepdog run -- …` leaves `job` as sheepdog's child. A subreaper supervisor would adopt it and its orphans as members and kill them (measured, review round 3). So if sheepdog starts with children (checked atomically with `waitid(…WNOWAIT)`), it forks once, before anything else: the child is a fresh supervisor, and the original process becomes a **relay**:
    - it forwards **TERM** (a TERM that also reached the supervisor through the process group only means "end the job" twice);
    - it forwards **HUP only when it is the session leader** (then only it gets HUP when the terminal closes);
    - it **never forwards INT**: a terminal INT already reaches the whole process group, and forwarding would deliver it twice (review round 4). It consumes INT, so it does not die of it. An INT sent to the relay's pid only therefore reaches nothing: the stated pid-only limit, stricter on this path;
    - it waits on SIGCHLD (no polling delay) and **dies the way the supervisor died** (death by signal N stays death by signal N, which shells rely on);
    - the supervisor has `PR_SET_PDEATHSIG(SIGTERM)`: if the relay is killed, the supervisor ends the job (measured: the root and an escapee are gone) rather than run on unseen.
    - **Stated limits:** the relay's own death by a core-dumping signal (QUIT, SEGV) does not report "core dumped" (it re-raises with RLIMIT_CORE 0; with a pipe `core_pattern` such as systemd-coredump the kernel ignores that limit and a second crash record is possible). If the caller ignored or blocked TERM, the parent-death TERM does nothing, and a killed relay leaves the supervisor running until the root exits.
    - Measured (Alpine): the relay path's exit latency equals the direct path's; the root's signal mask is identical on both paths.
- **After that the supervisor never forks again,** so it can still kill at pid exhaustion. It is single-threaded.
- **Reaping:** `waitpid(-1, WNOHANG)` in the event loop reaps every adopted orphan on Linux. Only the root's exit sets the exit code.
- **INT and HUP: forward only to members outside the supervisor's own process group** (the escapees), whatever the terminal state.
  - **Why:** a signal from the tty, and any signal sent to the **group** (`kill(-pgid)`, bash `kill %1`, bash's HUP on exit, `timeout(1)`'s `kill(0, sig)`), already reaches every process in the group. So the rule gives **exactly one delivery** to every member in every case: foreground, background, no controlling tty, or terminal closed. It needs no terminal state at all. Rounds 3–5 each found a double or missing delivery in rules that did depend on it; the kernel makes a tty signal and a `kill()` look the same (round 4), so no such rule can be correct.
  - **Exception: sheepdog is the session leader** (`ssh -t host 'sheepdog run …'`, `tmux new 'sheepdog run …'`, `script`, node-pty). When the terminal closes, the kernel sends HUP to the session leader **only**, never to the foreground group (measured on both OSes in round 6). So a supervisor that is the session leader forwards HUP to **every** member.
  - **The root is spawned into the supervisor's process group** (no `SETPGROUP`). The rule depends on this. If the root later moves itself to another group (`setpgid`, or `bash -c 'set -m; …'`), it becomes an escapee for forwarding, and ctrl-C/ctrl-Z from the tty may then not reach the supervisor at all. **Stated limit.**
  - **Stated limit, the same in every state:** an INT or HUP sent to the sheepdog **pid only** reaches the escapees, not the root. The advice has one form: to interrupt, signal the **group**; to end the job, send **TERM**.
    - **Callers in this class** (§4.1): Node `child.kill('SIGINT')`, `docker stop` with `STOPSIGNAL SIGINT`, npm forwarding with no tty. If such a caller then escalates with SIGKILL **to the sheepdog pid**, the supervisor dies without killing, and the tree waits for `sweep`. `--forward-int-to-root` is an opt-in for these callers; it double-delivers when a group signal also arrives.
  - **Hint when the limit bites (DevEx D9):** if the root is still running 3 s after the supervisor got INT or HUP, it prints once per job: `sheepdog: got SIGINT; the command is still running. A signal sent only to sheepdog's pid does not reach it: send TERM to end the job, or signal the process group (kill -INT -<pgid>).` The real pgid is filled in; `--quiet` suppresses it. Known false hint: a real ctrl-C whose cleanup takes over 3 s. It is **not** printed when sheepdog forwarded the signal to the root itself (the session-leader HUP case, or `--forward-int-to-root`). The auto-sweep line and this hint also appear in `--status-fd` (`notes: [...]`).
  - **Stated limit:** a caller that signals the pid **and** the group (GNU `timeout`, npm/pnpm `run-script` in a tty) gives escapees two deliveries.
- **TERM** to the supervisor means "end the job": it runs the full kill (§3.3). A TERM sent to the group also reaches the members directly; that is one TERM each plus the kill sequence, which is what the caller asked for. **Built in phase 0:** the supervisor catches TERM, kills the whole tree including the root, then dies of SIGTERM itself, so the caller sees death by signal. The handler is installed only when the caller left TERM at default: a caller that ignores TERM keeps it ignored, for sheepdog and for the root.
- **Job control (ctrl-Z):**
  - **The signal handler only increments a counter and writes a self-pipe byte.** The work runs in the event loop, never in the handler (no allocation in a handler). The loop reads the counter **just before** its own stop: events counted up to that point are one stop. An event after the resume is a new stop (a second ctrl-Z right after `fg` must work). The kernel itself discards a stop that is pending while a process is stopped (round 5).
  - **Orphaned process group: the kernel decides, sheepdog does not guess.** For the self-stop, the supervisor sets TSTP to the default action and does `raise(SIGTSTP)`. Both kernels discard a default TSTP **at delivery** when the group is orphaned (measured in round 6), and stop the process when it is not. If the stop was discarded, the supervisor CONTs the members it stopped and keeps running, and `--timeout` keeps counting. This replaces the r6 parent check. That check was wrong for npm, make and `sh -c` (the parent is in the same group), and it raced with a parent that dies during the wait (round 6).
  - **The members inside the supervisor's group** got the TSTP from the tty. They stop by themselves, and the event loop waits (bounded, 1 s) until they reach state T. So a pager or REPL can restore the terminal in its own TSTP handler (round 4: SIGSTOP would interrupt that handler). After the wait it SIGSTOPs the members that are still running. **Deliberate:** that includes a member that ignores TSTP, because sheepdog never leaves a member running while its own enforcement is stopped.
  - **The members outside the group** (the escapees) get SIGSTOP at once.
  - It records who was already in T, then does the self-stop above. On CONT it continues only the members it stopped.
  - TSTP is ignored while the kill loop runs.
  - **Stated limit:** a bare SIGSTOP of the group (not TSTP) stops the supervisor without any handler, so escapees keep running until CONT.
  - `--timeout` counts **running time only**: the supervisor measures the gap around its own stop (verified in round 5). In an orphaned group the stop is discarded, so all time counts.
- **Exit codes** follow `timeout(1)`, and **a machine-readable status carries the detail** (exit codes are ambiguous by nature: a command can exit 124 or 125 by itself):
  - the command's own code, or 128+n if it died from signal n;
  - 124 a sheepdog **limit** fired (timeout or cap), **143 the job was ended by TERM from outside** (128+15, as GNU `timeout` does; `trigger: "term"`), 125 sheepdog could not do its job (a usage error, or the kill deadline passed with members alive), 126/127 command not executable / not found;
  - `--status-fd N` / `--status-file PATH` writes one JSON line: `{"v": 1, "job": "7f3a", "root": "exited|signaled", "code": …, "trigger": null|"timeout"|"cap"|"term", "trigger_at": …, "deadline_missed": bool, "killed": [{"pid", "cmd", "escaped": null|"setsid"|"reparented"|…}], "survivors": [...], "tracking": "responsibility|puniq|subreaper", "degraded": null|"<reason>", "error": null|"…"}`. `tracking` and `degraded` say how complete the membership was: "no survivors" means "clean" only when `degraded` is null (outside voice F2). Machine callers (liveapp, ccwho) must read it and not the exit code. The status fd is set `CLOEXEC` after the SETEXEC (like the lock), so no member can hold it open or write a false line;
  - **exit-code order:** 125 (error, or deadline missed) > 124 (timeout or cap) / 143 (external TERM) > the command's own code. When timeout and cap both fire, the **first** is the `trigger`;
  - a one-line reason goes to stderr.
- **Lock:** it marks the job as live for `sweep`. It is taken **after** the SETEXEC (a CLOEXEC fd would be closed by it) and is `O_CLOEXEC`, so the job never inherits it.

### 3.2 Membership: lineage facts only, and sticky

A process is a member only with a **lineage fact**. Session, pgid and env tag are never membership; `ps` shows them as suspects only.

| Fact | Platform |
|---|---|
| the root | both |
| `responsibility_get_uniqueid_responsible_for_pid(p)` ∈ **R**, where R is a set of **uniqueids**: {the supervisor} ∪ {**members** that are responsible for themselves, e.g. an inner sheepdog or an app that disclaims again} | macOS |
| `p_puniqueid` ∈ the **known set** = the uniqueids of every member ever seen (so it includes R) (second fact; catches descendants that disclaimed again) | macOS |
| ppid chain to the supervisor (the subreaper) | Linux |
| a journal entry with the same boot id and the same uniqueid / start time | both, for `sweep` and `kill` |

**Rules:**
- **Sticky.** Once a uniqueid (macOS) or (pid, start time) (Linux) is known as a member, it stays a member while it is alive. A later loss of evidence does not remove it.
- **−1 means "no fact".** It is never an error and never a match (other users' processes, zombies).
- **Same uid** on every fact. The supervisor, its ancestors and the caller's shell are **never** members.
- **R only grows from members.** A uniqueid enters R only if its process is already a member by a lineage fact. A member that becomes responsible for itself joins R while it is a member, so its descendants are caught without a race (round 3: without this, a re-disclaiming member's stray was lost after the member exited).
- **Nesting is registered, and the kernel says who registers (macOS only).** On Linux the subreaper chain already holds every nested job, so no registration is needed. An inner `sheepdog run`, **before** it disclaims:
  1. reads `SHEEPDOG_OUTER`: the **whole chain** of enclosing supervisors (socket path + per-job nonce each). It registers with **every** one of them, in series, within one 2 s budget for all, each doing its own check, so that O → A → B leaves B reachable from O even when A is killed first. Then it appends its own entry to the chain for its job. Past 16 entries it does not append and warns (a deeper job is still found by the scan). An env path survives Node's and Python's spawn, which close inherited fds (measured in round 3), so no fd is inherited.
  2. It sends a fixed-size record with the nonce. The outer supervisor then reads the peer identity from the **kernel**: `LOCAL_PEERTOKEN` (the audit token, measured in round 4: unprivileged, and it fails once the peer is dead). The identity is taken **after** the record is read, because the kernel names the process that last wrote. It compares the token's `pidversion` with the live `p_idversion` before and after the membership check, and adds the **uniqueid** to R, never the pid. So a process can only register **itself**.
  3. The outer checks that the peer is the same uid and **is already a member**, using the **same membership function as the scan** (R, `puniq` or sticky). Only then does it add it to R and send one ack byte.
  4. The inner waits for the ack (bounded, 2 s), then disclaims. Without an ack it warns on stderr and continues. The outer still adds it at its next scan, if it is a member by then: the loss window is one 250 ms poll.
  - Anything malformed is rejected and logged.
  - **Socket:** in a fresh `mkdtemp` directory (0700) under `$TMPDIR`. If that path would pass 100 bytes (`sun_path` is 104 on macOS; liveapp hit this with tsx), under `/tmp`. A `mkdtemp` name cannot be taken first by another user. The path travels in the env, and the inner uses it verbatim. If `bind` fails, the job runs unregistered-to (inner jobs are then found by the scan) and sheepdog warns once. The listening fd is `O_CLOEXEC`. sheepdog does **not** use `POSIX_SPAWN_CLOEXEC_DEFAULT`, because the job must inherit the caller's fds (liveapp passes fd 3).
  - The supervisor handles `accept` in its event loop, with a 100 ms read deadline per connection, so a silent client cannot stall it. It does not fork.

### 3.3 The kill: snapshot, freeze, verify, kill, repeat, with a deadline

0. **Snapshot** the member closure **before** any signal.
1. **TERM phase** (`--grace`, default 2 s): TERM every member, then CONT (a member stopped by job control would not see the TERM). Keep tracking (sticky). `sheepdog kill` against a supervisor that is stopped sends TERM and then CONT to the supervisor.
2. **Freeze:** SIGSTOP every member, after an identity check:
   - **Linux:** `pidfd_open`, then re-read the start time, then signal through the pidfd. If pidfd returns `ENOSYS`/`EPERM`: re-read the start time, then `kill`, for STOP and KILL alike.
   - **macOS:** re-read the uniqueid.
   - Record which members were already stopped (state T).
3. **Close:** recompute until stable; stop new members the same way.
4. **Verify:** re-check identity. A process that fails the check gets SIGCONT only if it was not in state T before. **macOS limit (stated):** in a pid-reuse case, that T record describes the old process. The window is milliseconds and pids are sequential; this is accepted and named. A test seam injects the failure so that the cell can run.
5. **Kill:** SIGKILL all.
6. **Repeat** 2–5 until two consecutive scans find zero members (Linux: and zero unreaped zombies whose ppid is the supervisor), **or until the kill deadline** (`--kill-deadline`, default 10 s). At the deadline: exit 125, and print each remaining pid with its state (D state, still exiting, uid changed by a setuid exec).

This runs on **every** job end. With `--leave-strays` it is skipped, the journal marks the job, and `sweep` never touches it.

### 3.4 Caps

The supervisor polls every 250 ms:
- `--max-mem`: sum over members of `phys_footprint` (macOS) or RSS from `statm` (Linux).
- `--max-procs`
- `--timeout`

macOS has no working `RLIMIT_AS`, so on a Mac this poll is the only memory limit.

### 3.5 Journal and `sweep`

- **Header:** the boot id, the Linux pid-namespace inode, the owner tag (`--owner`) and the leave-strays flag.
- **Lines:** one `write(2)` per scan, **before** any signal to that scan's new members. There is no fsync: a reboot makes the journal invalid anyway (boot id), and `write` survives a SIGKILL.
- **Location:** `$STATE/jobs/<boot-id>-<pidns>/<job-id>.journal`. A host and a container that share `$STATE` through a bind mount therefore never see each other's journals (round 7 measured that a host `flock` is not visible to a container through a Docker Desktop bind mount).
- **A journal is never visible unlocked:** the supervisor creates it under a temporary name, takes `flock(LOCK_EX)`, then renames it into place.
- **`sweep --owner X`** acts only on X's stale jobs: the sweeper **takes** the job's lock with `LOCK_EX|LOCK_NB` and holds it until the kill and the journal delete are done. Held by another = live, skip. **Any other lock error also counts as live** (fail safe). It skips leave-strays jobs. A journal from another boot or pid namespace is **skipped, never deleted**: it lives in another folder anyway.
- **What `sweep` can find:**
  - **macOS:** journal entries that still match, plus the live `p_puniqueid` closure from them. Responsibility is **gone** once the supervisor is dead. `puniq` resets on exec after reparenting, so a daemon that the journal never saw is lost. **Stated.**
  - **Linux:** journal entries that still match, plus their live ppid children. Orphans of unjournaled intermediates went to PID 1 and are lost. `PR_SET_PDEATHSIG(SIGKILL)` on the root narrows this. **Stated.**
- **Auto-sweep (DevEx D8):** every `sheepdog run` first sweeps the **same owner's** stale jobs, bounded to 200 ms, with every guard above (boot id, pid namespace, lock not held, leave-strays skipped). It prints one line when it swept something: `sheepdog: swept 1 dead job (pid 4242 python3 held 3 processes)`. `--no-sweep` skips it. So a supervisor killed with SIGKILL is cleaned up at the next run, and the agent needs to know nothing.
  - **Completion (outside voice F3):** the auto-sweep skips the TERM grace (the members of a dead job are leaks, not a live job): verify, STOP, KILL. If the 200 ms budget runs out, the rest waits for the next run, the journal is kept, and the line says `partial`. The new command then starts; it never waits for a slow sweep.
  - **Owner:** `--owner` defaults to `default`. A caller that wants its sweeps isolated passes its own name (liveapp: `--owner liveapp`).
- **So `sweep` is a backstop, not a guarantee.** The guarantee is the supervisor alive. The supervisor itself is small and never forks, so the usual reason it dies is a SIGKILL from the caller. The README tells callers to send TERM.

## 4. Limits, stated plainly

### 4.1 Not contained

- **Other launchers start the work.** On macOS: `open`, `osascript`, `xcodebuild test`/simctl, launchd agents and `launchctl submit`. On Linux: `systemd-run --user` and D-Bus activation. These are not killed and not counted in `--max-mem`. The README and `ps` say so. Cell 17 documents it.
- **A descendant disclaims again** (Electron, VS Code or Chrome helpers started inside the job). Only `puniq` can catch it, with the race above.
- **Callers that send INT to the pid only, then SIGKILL the pid** (Node `child.kill`, `docker stop` with `STOPSIGNAL SIGINT`): the root does not get the INT, and the SIGKILL kills only the supervisor. The tree then waits for `sweep`. Callers should send TERM (§3.1).
- **Deliberate escape:** `sudo`, another uid, ptrace. sheepdog is not a sandbox.
- **Docker containers** started by the tree belong to `dockerd`. Out of scope for v1.
- **Windows:** not in scope.

### 4.2 Where sheepdog is weaker than cgroupfs

cgroupfs membership cannot be left at all, and the kernel accounts memory. Where cgroup v2 is delegated (EOS), cgroupfs stays first. sheepdog is the **portable floor**, and the **only** option on a Mac.

### 4.3 The macOS private SPI

The responsibility calls are private SPI. They are exported, and Apple's own tools use them. At startup `sheepdog` resolves `responsibility_spawnattrs_setdisclaim`, `responsibility_get_uniqueid_responsible_for_pid` and `responsibility_get_pid_responsible_for_pid` (the last one for `ps` only) with `dlsym` and runs a self-check (the SETEXEC re-exec reports itself as responsible). If this fails, sheepdog falls back to `puniq` only, prints a loud warning, and `doctor` reports the degraded mode. A cell forces the missing-symbol path.

### 4.4 macOS: privacy (TCC) grants

- A disclaimed sheepdog becomes the "responsible app" for TCC. The job **does not inherit the terminal's grants** (Full Disk Access, Documents, Desktop, Downloads, camera and so on).
- **Measured 2026-09-25 (DevEx review; the operator toggled the grants):**

  | Case | Result |
  |---|---|
  | bare binary, granted, original path | allowed |
  | identical bytes at a **new path** | **denied**, and a second Settings row appears (grants are keyed to the **path**; yabai users hit this on every `brew upgrade`) |
  | the same binary inside an `.app` bundle, granted | allowed; the row is the **bundle ID** (`client_type 0`), even when the CLI is run directly |
  | the bundle **moved** to another folder | **allowed** (the grant follows the bundle ID) |
  | the bundle **re-signed ad-hoc** (new hash) | **denied**, and the grant was **reset to off** |
  | a bare binary after one denied access | listed in Full Disk Access (operator checked) |
  | a bundle in a temp folder, added with `+` | the grant is stored and works, but the row is **not shown** in the list |

  Homebrew **formula** binaries are ad-hoc signed (`jq`, `gh`, `rg`: TeamIdentifier not set), so a formula would lose the grant on every upgrade. Homebrew **cask** artifacts keep the vendor's Developer ID (`1password-cli`: team `2BUA8C4S2C`). **Inferred, not measured:** a Developer ID build stores "bundle ID + team" (the form in the operator's Claude Code row), so release N+1 matches release N's grant.
- **So (DevEx D12, reopened; supersedes the stable-path design):** the macOS release is **`Sheepdog.app`** (bundle ID `com.lukaso.sheepdog`, Developer ID-signed and notarized) with the CLI as its executable. Homebrew delivers it as a **cask** (`brew install --cask lukaso/tap/sheepdog`, with a `binary` link into PATH). npm and `install.sh` each install their own copy of the bundle plus a PATH link. There is no shared file, so no cross-installer ownership rule is needed. The SETEXEC self-re-exec uses the bundle's executable path (`proc_pidpath`); round 7 measured that it resolves PATH symlinks into the bundle. A **hardlink** keeps its own path, so installers must link with symlinks (pnpm's hardlinks are not allowed for the bin). If the re-exec fails (for example `brew upgrade` removed the old version mid-start), sheepdog continues in the fallback tracking with the §4.3 warning instead of failing the job. `doctor` reports whether the grant is present.
- **Phase 0 result, 2026-09-25 (operator's Developer ID, team P7UM972E39):** macOS stored the grant's requirement as `identifier "com.lukaso.sheepdog-probe2" and anchor apple generic and … certificate leaf[subject.OU] = P7UM972E39`, with **no build hash**. A rebuilt binary (different bytes, same identity) that replaced the granted one was **allowed**, and so was a copy in another folder. A bundle in `~/Applications` was **listed** in Full Disk Access with no `+` step. No keychain dialog appeared: the key's access list already allowed `codesign`, and no password was requested.
- **Phase 0 finding: a mismatching build with the same bundle ID revokes the grant.** A fresh ad-hoc build with the same bundle ID was denied (the control), and running it **reset the grant to off** for the correctly signed build too. So:
  - **Dev and CI builds use a different bundle ID, `com.lukaso.sheepdog.dev`.** Only the release packaging job writes `com.lukaso.sheepdog`, so a local `cargo` build can never switch off a user's release grant.
  - **A fork can revoke, never gain.** A fork's build cannot match the team requirement, so it cannot gain access, but if a user runs a fork's build under the release bundle ID, their grant is switched off. `doctor` detects a lost grant and names this cause.
- **Remaining phase-0 checks:** (1) done (above); (2) the grant holds when the CLI runs through the PATH **symlink** (the test was inconclusive because test 5 had reset the grant); (3) whether a bundle in its install location (Caskroom, npm prefix) shows in the Settings list, or whether the `+` path must be given. Measured 2026-09-25: `tccutil reset … <bundle id>` fails with OSStatus -10814 ("application not found") while the bundle sits in `/private/tmp`, even after `lsregister -f`, and succeeds from `~/Applications`. Launch Services does not index temp folders, which likely explains why the probe's row never showed in Settings. So the install location must be one Launch Services indexes (e.g. `~/Applications`, `/Applications`, Caskroom), and `sheepdog doctor` and the uninstall docs rely on it (`tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog`).
- **Start-time warning (DevEx D10):** on macOS, when the cwd or an argument path is under a protected folder (Documents, Desktop, Downloads, iCloud Drive, removable/network volumes) and a probe `opendir` fails, `run` prints once: `sheepdog: ~/Documents is privacy-protected and sheepdog may not read it, so this job cannot either. Give sheepdog Full Disk Access: System Settings > Privacy & Security > Full Disk Access, click +, and choose /path/to/Sheepdog.app (press ⌘⇧G to paste the path). Or run with --inherit-terminal-permissions (weaker tracking).` The real bundle path is filled in. The `+` route is given because the list may not show the bundle row (measured). Known miss: a protected path the job reaches from a config file.
- `doctor` reports which grants sheepdog has.
- `--inherit-terminal-permissions` (DevEx D11; was `--no-disclaim`): the job gets the privacy permissions of the app sheepdog was started from (usually the terminal), and sheepdog falls back to `puniq` tracking, with the race of §2. Validated 2026-09-25: without the self-disclaim the job's responsible process is the terminal (iTerm2) and a protected read succeeds. It warns once. On Linux it is accepted and does nothing.
- Cell 19 runs the same protected read inside and outside sheepdog and checks the documented result.

## 5. Language and build

**Rust.**
- Static musl binaries on Linux (any container).
- Direct syscalls through the `libc` crate; `dlsym` for the macOS SPI.
- No GC threads.
- Cross-compiled from this Mac: `cargo-zigbuild` for `{x86_64,aarch64}-unknown-linux-musl`, native `{aarch64,x86_64}-apple-darwin`, `lipo` for a universal Mac binary.

**Why not Go:** the macOS calls need cgo, and cgo removes Go's easy cross-compile.

### 5.1 Release security (public repo, signed releases)

- **Signing and notarization run only** on a `v*` tag pushed to `lukaso/sheepdog` (`if: github.repository == 'lukaso/sheepdog'`), in a protected GitHub **environment** `release` that requires the operator's approval.
- **The Developer ID certificate, its password and the notarization credentials** are environment secrets of `release` only, never repository-wide secrets.
- **No workflow uses `pull_request_target` or `workflow_run` on fork events.** Fork PRs run the test matrix only; GitHub gives them no secrets.
- **Forks build ad-hoc-signed binaries, with the `.dev` bundle ID.** A fork cannot produce a binary that matches the team requirement, so it cannot inherit a user's grant. It can revoke one if run under the release bundle ID (measured in phase 0, §4.4), which is why only the release job writes that ID.
- **The build never sees a secret (round 7).** The cargo build runs in a job with no secrets and uploads the unsigned artifacts. A separate job in the `release` environment only downloads, signs and notarizes them. So no dependency's `build.rs` or proc-macro runs while the signing keychain is unlocked.
- **npm publishes by OIDC trusted publishing** (no stored token). The Homebrew tap push uses a fine-grained token stored only in the `release` environment.
- **The `release` environment's deployment rule** allows only tags `v*`. **Every third-party action is pinned by commit SHA.**
- **Free CI:** GitHub Actions is free for public repositories on standard runners, so the billing block on liveapp's CI does not apply here (§6's platform-matrix note predates the Q3 decision).

## 6. Tests

TDD per the global rules. Every cell that claims sheepdog **catches** something has a control or mutant that must come out the other way. Cells 17 and 19 only **document** a limit; they assert the documented result.

**The checker.** Each process the test starts writes its own (pid, uniqueid or start time) to a file *before* it tries to escape. The checker verifies exactly those identities, never `ps` by marker. **It runs after the root has exited and been reaped** (review P1-B): that is the state where evidence can disappear.

**Escape cells:**

| # | Route | Control (must leak) |
|---|---|---|
| 1 | deep tree (depth 50) | kill of the root only |
| 2 | grandchild calls `setsid` | group kill |
| 3 | **root forks a `setsid` double-fork escapee (intermediate exits in < 1 ms, then exec of `/bin/sleep`) and exits at once** | macOS: the root-disclaim design (r2); Linux: no subreaper |
| 4 | `nohup … & disown` | shell exit |
| 5 | child ignores SIGTERM | TERM only |
| 6 | stopped child, then its parent dies | TERM |
| 7 | fork storm, 200/s for 3 s; then 0 zombies whose ppid is the supervisor | a one-pass kill; mutant: no orphan reaping |
| 8 | `/bin/bash` loop (**macOS only**: on Linux the env tag is readable, so this control would not leak) | env-tag sweep |
| 9 | `env -i` descendant | env-tag sweep |
| 10 | pty via `script` | group kill |
| 11 | `bash -c 'set -m; "$@"'` (#332) | `kill -pgid` |
| 12 | clean root exit leaves a stray | wait for the root only |
| 13 | python allocates ~50 MB/s, `--max-mem 500M`: dies below 1 GB | same run without `--max-mem` passes 1 GB (bounded by the test's own `timeout -s KILL`) |
| 14 | supervisor SIGKILLed, then `sweep` finds the **journaled** members | mutant: sweep without the journal |
| 15 | **macOS:** inner `sheepdog run` started through a fast double-fork, and also through a Node and a Python launcher; outer killed; 0 inner survivors. **Also three levels, O → A → B, all through fast double-forks: 0 survivors in B's subtree.** **Linux:** the same trees, 0 survivors (subreaper; no mutant) | mutants (macOS): no registration; register with the nearest supervisor only |
| 16 | the 2026-09-25 harness shape inside sheepdog | the same shape without sheepdog |
| 17 | `open -g` helper (macOS) | documents the leak: the cell asserts that it survives |
| 18 | TERM to the process the caller launched (npm-installed `sheepdog`) ends the tree | a node wrapper that does not exec |
| 19 | protected read (TCC) inside and outside sheepdog | documents the result (§4.4) |
| 20 | an unkillable member (test seam) ends at the kill deadline with exit 125 | mutant: no deadline (bounded by the test's `timeout`) |
| 21 | ctrl-Z in a pty: (a) an escaped member's tick counter does not advance while the job is stopped; (b) an **escapee** the user had stopped before stays stopped after `fg`; (c) a same-group root with a 300 ms TSTP handler finishes it: the marker exists **while the job is stopped**; (d) the root's TSTP handler calls `kill(0, SIGTSTP)`, then `fg`: the job keeps running; (e) orphaned group: `setsid sheepdog run --timeout 2s -- sleep 60` plus `kill -TSTP` ends at ~2 s with trigger `timeout` | mutants: default TSTP handling ((a) ticks); CONT all ((b) resumes); SIGSTOP the group at once ((c) no marker while stopped); no coalescing ((d) stops again); stop even when orphaned ((e) hangs, bounded by the test's own `timeout`) |
| 21′ | (f) `sh -c 'sheepdog run …; true'` in a pty, ctrl-Z: the escapee stops ticking and the prompt returns; (g) the parent is killed while the job waits to stop: the job ends at `--timeout`, no hang; (h) ctrl-Z again 0–10 ms after `fg` stops the job again | mutants: the r6 parent-only orphan check ((f) keeps ticking; (g) hangs, bounded); clear the counter after the resume ((h) not stopped) |
| 22′ | (g) sheepdog is the **session leader** of a pty (`setsid` + `TIOCSCTTY`), the master is closed: the root counts exactly 1 HUP, escapees 1; (h) the pid-only limit plus escalation: INT then SIGKILL to the sheepdog pid, then `sweep` finds and kills the journaled members | mutants: no session-leader exception ((g): the root counts 0); sweep without the journal ((h) leaks) |
| 22 | each member counts **exactly one** INT (or HUP): (a) foreground pty, stdin from `/dev/null`, ctrl-C; (b) background job, `kill -INT -<pgid>`; (c) no controlling tty (as liveapp and CI run), `kill -INT -<pgid>`; (d) terminal close, HUP; (e) an escapee counts one for each of (a)–(d); (f) the stated limit: INT to the sheepdog **pid only**: the root counts 0, escapees 1 | mutants: forward to all members ((a)–(d): the root counts 2); forward to none ((e): escapees count 0) |
| 23 | the root's SIGPIPE disposition and signal mask equal a run without sheepdog | mutant: no SETSIGDEF/SETSIGMASK (`yes \| head -c1` prints Broken pipe) |
| 24 | a member reached through a double-fork **and** an exec after reparenting (so its `puniq` is 1: only stickiness keeps it) re-disclaims after it was observed, forks a stray and exits; 0 survivors | mutants: no stickiness; no R-growth from members (each loses the stray) |
| 25 | `TMPDIR` longer than 100 bytes: the inner gets its **ack** (no warning on stderr) | mutant: no `/tmp` fallback (bind fails: the warning appears) |
| 26 | exit status: a command that exits 125 by itself, a usage error, and a deadline with a live member give three different `--status-fd` lines; an external TERM exits 143 with `trigger: "term"`, a timeout 124; `kill` of an already-gone pid exits 0, a refused pid 1; timeout and cap firing together record the first; the root has **no** fd N open | mutants: status from the exit code only; the status fd not CLOEXEC |

**No-setup mode cells (§3.0):**

| # | Case | Control / mutant |
|---|---|---|
| 27 | `kill <pid>` of a live tree, depth 20: 0 survivors | mutant: kill of the target only |
| 28 | (a) a `setsid` grandchild whose parent is **alive**: proved by the ppid chain on both OSes and killed (on macOS the mutant also removes the `puniq` closure, since `puniq` proves the same grandchild); (b) a `setsid` grandchild whose parent **has exited** (reparented): **macOS** proved by `puniq` and killed; **Linux** listed as a suspect, alive without `--include-suspects`, killed with it | mutants: no ppid closure ((a) survives); no `puniq` closure ((b) macOS survives); suspects killed by default ((b) Linux: a sibling decoy dies) |
| 29 | (a) `kill <pid>` on a **supervisor** ends its whole job, including an escapee that `kill` alone could not prove; (b) `kill <pid>` on a **non-root member** kills only that member's subtree, the job's root and other workers stay alive, and the output names the job (a root target ends the job by design); (c) `strays --kill --older-than 1s` skips a stray that is an escapee of a running job; (d) an **app-helper decoy** (a double-forked helper inside a live app bundle) is not listed by `strays` and survives `--kill`, while the 2026-09-25 shape is listed; (e) Linux container with `sh` as PID 1: its live child survives `strays --kill`; (f) `kill <outer pid>` with an inner `sheepdog run` in the tree: 0 inner escapees survive | mutants: no job kill for a supervisor ((a) escapee survives); hand-over for members ((b) the root dies); no live-job skip ((c) the job's escapee dies); no responsibility rule ((d) the helper is listed and dies); no PID-1-child skip ((e) the child dies); freeze at once ((f) inner escapees survive) |
| 30 | `strays` lists the **2026-09-25 shape** (subshell killed, `python3` orphaned) with its origin; **macOS:** does not list an `open -g` helper (`puniq` 1) | mutants: no orphan filter (the helper is listed); no `puniq ≠ 1` rule (the helper is listed) |
| 31 | `strays --kill` without a filter refuses; in a non-tty without `--yes` refuses; with both kills only the matching rows | mutant: no filter required |

**Safety cells.** Each decoy must survive, and each has a mutant that kills it:
- **no-setup:** a same-uid process in the target's process group, started **before** the target, is not a suspect. Mutant: no start-time rule.
- **no-setup:** `kill 1`, `kill $$` (the caller's shell) and another user's pid are refused, and nothing is signalled. Mutant: no target checks.
- **registration spoof (macOS):** a member connects and tries to register for a responsible-to-terminal decoy; the kernel peer identity is the member itself, so the decoy survives. Mutant: trust an id in the record (kills the decoy, and with it every process responsible to the terminal: 158 on this Mac).
- **a non-member registers (macOS):** the decoy is **another sheepdog job's supervisor** (responsible for itself, with its own tree). It connects with a stolen nonce and is rejected, so its tree survives. Mutant: no membership check (the decoy joins R, and its tree dies).
- (Deleted in r5: the "fd passing" cell. Under its mutant it is the member, not the non-member, that joins R, so no decoy can die and the cell cannot go red. The barrier it meant to test is the membership check, which the cell above asserts. The r4 rule "read the identity after the record" stays, for correctness of the pidversion check.)
- **`--inherit-terminal-permissions` and the fallback path:** a process in another terminal tab, responsible to the same terminal app, survives. Mutant: R = {responsible(supervisor)}.
- a sibling command in the same script: `sheepdog run -- a & b; wait`
- a `| tee` partner
- two concurrent jobs from one shell
- another user's process (Linux in Docker; macOS returns −1)
- a journal entry pointing at a live decoy with a wrong identity: zero signals, checked by the decoy's own signal counter
- a journal from another boot id, and one from another pid namespace
- the wrong-freeze rollback: a pre-stopped decoy stays stopped, a running one gets SIGCONT (test seam)
- the caller's shell and the supervisor's ancestors
- the forced pidfd `ENOSYS` path, for STOP and KILL

**Deleted in r4:** the r3 safety cell "a lineage fact but started before the job". It tested a start-time guard that r3 had already removed. Its invariant (a process from before the job is never a member) now lives in the rule that **every fact is a lineage fact** (§3.2): a pre-existing process cannot have one. The safety cells "sibling command" and "same-uid process started before the job" assert it.

**Mutation cells:** remove each membership fact, stickiness (cell 24), R-growth from members (cell 24), the snapshot-before-TERM (reachable only in `--inherit-terminal-permissions` mode: cells 2, 3 and 12 run in that mode too), the freeze, repeat-until-empty, orphan reaping, the boot-id check, registration (cell 15), the registration membership check, and the deadline. Each must turn a named cell red, and each mutant must be proven to have run.

**Stress:** cell 3, 10 000 runs, the checker after the root is reaped. **0 lost** with the supervisor-responsible design. The root-disclaim mutant must lose more than 0 in the same run.

**Platform matrix** (local `./test-all`, because GitHub Actions is blocked by billing):

| Target | Where |
|---|---|
| macOS arm64 | this laptop |
| macOS x86_64 | a CI Intel runner if one exists (unverified). Otherwise untested, and stated. |
| Linux arm64 / amd64, default container, not PID 1 | Docker Desktop |
| Linux as PID 1, and under `--init` | Docker Desktop |
| Alpine (musl) and Debian (glibc) | Docker Desktop |
| Linux on a real host | the Hetzner box as `ghrunner` (it shares the host with EOS: no stress without asking) |
| gVisor, WSL, rootless Docker, old seccomp | unverified; the pidfd fallback covers them |

**Distribution cell:** a browser download of the release binary runs (Gatekeeper quarantine).

## 7. Phases

| Phase | Output | Gate |
|---|---|---|
| **0. Spike** (1–2 days) | Rust ports of the probes; SETEXEC self-disclaim; cell 3 stress with the checker after reaping; socket registration with kernel peer identity; pidfd fallback; **TCC (§4.4 phase-0 checks): a Developer ID rebuild keeps the grant; the bundle row in the install location** | **0 of 10 000** lost after the root is reaped, **and** the root-disclaim mutant loses > 0. If not: stop and decide. |
| **1. Core** | `run`, the kill on every exit, signals and job control, `kill <pid>` (proved members, `--dry-run`); cells 1–12, 16, 20–24, 27, 28 (proved part), the no-setup safety cells; safety cells: sibling command, `\| tee` partner, two concurrent jobs, another user, the caller's shell and ancestors, the wrong-freeze rollback, the pidfd `ENOSYS` path | green on the Mac and both Docker arches; every control and mutant **that applies to the platform** red |
| **2. Caps + journal + registration + orphans** | caps, `--status-fd`, journal, registration, `sweep`, `ps`, `kill <job>`, suspects and the hand-over in `kill <pid>`, `strays`, `doctor`, `--inherit-terminal-permissions`; cells 13–15, 17, 19, 25, 26, 28 (suspects), 29–31; safety cells: registration spoof, a non-member registers, `--inherit-terminal-permissions` other tab, the journal cells (wrong identity, other boot, other pid namespace) | as above (registration cells and mutants are macOS-only) |
| **3. Ship** | releases (Linux: 2 static binaries; macOS: `Sheepdog.app`, universal, Developer ID-signed + notarized; checksums), `install.sh`, a Homebrew **cask** in `lukaso/tap`, npm `@lukaso/sheepdog` installing its own bundle copy and a PATH link whose process is the bundle executable (cell 18), the README with the agent snippet (§10.6), the agent first-use eval (§10.9) | clean container, clean Mac user, the distribution cell; a grant survives an upgrade (install v1, grant, install v2, protected read works); the first-use eval passes |
| **4. Adopt** (each its own plan) | 1: the ccwho harness. 2: liveapp `check.sh`, the pty runner, the vitest gate. 3: a `sheepdog` backend in `reap-detect.ts` between cgroupfs and `none`. 4: `sweep --owner liveapp` at startup. 5: a **standard install for the liveapp fleet** (the container image and `liveapp init` provision sheepdog). | each adoption proves its own escape cell red before and green after |

### 7.1 Phase 0 result (2026-09-25): gate PASSED

| Check | Result |
|---|---|
| **Gate, macOS arm64** (macOS 26.6): cell 3, 10,000 iterations | **0 survivors** (39 min; fixture v1, root exits at once, ≥90% creation guard). Re-confirmed with fixture v2 (root records via a pipe, ≥99% guard): 1,000 iterations, 0 survivors |
| **Gate, Linux** (Docker Desktop, Alpine/musl, aarch64): cell 3, 10,000 iterations | **0 survivors** (19 min; final fixture, ≥99% guard). Debian/glibc: 300 iterations, 0 survivors |
| Controls in the same suite | no sheepdog: every escapee survives; macOS round-2 design (root disclaims): survivors; Linux without the subreaper: survivors |
| Mutants | macOS without the self re-exec: 20/20 survive; without the one-attempt guard: re-execs forever (caught at 5 s) |
| Developer ID grant across a rebuild | kept (§4.4); a same-ID mismatching build revokes it, hence the `.dev` bundle ID |
| Defects the spike found in itself | the uniqueid SPI **returns** the id (no out-parameter); a failed self re-exec looped (now at most once); a fixture recorder that sheepdog can kill undercounted creation (the root records now) |

**What the gate does and does not prove (phase-0 review, 2026-09-25).** In cell 3 the escapee stays responsible to the live supervisor (macOS) or comes back to it (Linux), so the outcome does not depend on timing: 10,000 iterations confirm the mechanism, they do not measure a race margin. The round-2 control lost 20/20 by design, not by chance. The race-sensitive parts are covered by their own cells after the review fixes:

| Rule | Cell | Mutant | Result |
|---|---|---|---|
| repeat until nothing is known alive | cell 7 (breeder), cell 3 | one pass, no repeat (M2) | **red** |
| sticky membership; empty only when every known member is confirmed dead; report at the deadline | cell 24-lite / cell 20 (seams: forget, no-kill, short deadline) | not sticky (M4) | **red** |
| the root keeps the caller's signal state | cell 23 (HUP, INT, TERM ignored; SIGPIPE default and ignored) | reset all to default (M1a); normal Rust `main` (M1b) | **red** |
| identity checker (not only argv markers) | control: an escapee with no marker is found by identity | — | green (5/5 found) |
| freeze (SIGSTOP) before kill | cell 7 (breeder, chain) | no freeze (M3) | **green: not load-bearing in any safe test.** A bounded tree converges by repeat alone; only unbounded fast growth (a fork bomb) separates them, which is not run on a workstation. **Named residual.** |

**Second review of the fixes (2026-09-25), 2 P1 + 2 P2 + 3 P3, all fixed test-first:**

| Finding | Fix | Cell | Mutant |
|---|---|---|---|
| P1-A Linux false clean: a tree that moves faster than one `/proc` scan looks empty twice | Linux ends on the **authoritative** check: `waitpid(-1, WNOHANG)` = ECHILD (the subreaper is the parent of the topmost live member). A scan-only "empty" never ends it while children exist | cell 7 fast chain (0 µs generations, 30 per run); 25 full-suite runs each on Alpine and Debian: 0 failures | scan-only end (M5): red 5 of 5 runs, unmutated green 5 of 5 |
| P1-B a caller that ignores SIGCHLD makes the kernel auto-reap; the supervisor never kills | sheepdog sets SIGCHLD to default before anything else; **the root gets SIGCHLD at default** (a stated exception to "inherit the caller's signals") | exec with SIGCHLD ignored: root's code within 5 s, escapee dead | no reset (M7): red |
| P2 non-UTF-8 argv corrupted | argv stays raw bytes end to end (also the macOS re-exec) | `\xff\xfe` reaches the root unchanged | lossy conversion (M8): red |
| P2 the deadline did not bound the runtime | deadline checked at the top of every pass; a 100 ms settle before "not clean" | debug seam NEVER_EMPTY: exit 125 within the deadline | deadline only in the non-empty branch (M6): red |
| P3 a panic unwinding out of the C `main` (UB before Rust 1.81); cleanup double-panic | errors written with ignored results, body under `catch_unwind`; the checker never unwraps `ps` | closed stderr with SIGPIPE ignored: exit 125 | both layers removed (M9b): red |

**macOS residual (named):** macOS has no atomic "tree empty" check (no subreaper, no enumeration of responsible processes), so it still ends on two empty scans. The fast chain is green there (fork is slow enough), which is luck, not proof. A process chain that hops faster than a scan can outrun it on macOS.

**Round 4 (review of the round-3 fixes): no P1; 2 P2 + 5 P3 in the new Linux relay, fixed test-first (death by signal preserved, no double INT by design, no exit latency, a dead relay takes the supervisor with it, the relay path keeps the caller's mask, atomic child check, a distinct panic message). Mutants MR1–MR4 red. The no-double-INT rule has no test until phase 1 gives the supervisor INT handling (cell 22, relay variant).**

**Round 5 (review of round 4): no P1; P2-A was a regression of round 4** (the relay's parent-death TERM killed the supervisor by default action and leaked the tree; the dead-relay cell hid it by cleaning the root itself). Fixed by building "TERM ends the job" now; **P2-B**: the mask cell measured nothing on Debian (dash resets the mask), now no shell in the path and the control must show the injected bits. Mutants MT1, MR4 (now on Debian), MT3b (50 ms polling), MT4a/b (HUP rules) red. Test lesson: count processes by program and marker, not by marker alone (the marker also sits in sheepdog's own argv).

**Carried into phase 1** (not measured in phase 0): the TERM grace (§3.3); the pid-reuse seam for the identity re-check (the check exists, the forced-reuse cell does not); the pidfd path and its `ENOSYS` fallback; socket registration (facts from the round-4 probe only); Linux on amd64 and as PID 1; the grant through a PATH symlink (the phase-0 grant was reset by the control before this could be tested).

## 8. Open questions for the operator

1. **Platforms:** DECIDED 2026-09-25 (operator): Linux + macOS for v1.
2. **Kill strays at every job end:** DECIDED 2026-09-25 (operator): yes by default; `--leave-strays` for tools that start a helper on purpose (Gradle daemon, `sccache`, `docker compose up -d`).
3. **Repo:** DECIDED 2026-09-25 (operator): public, `lukaso/sheepdog`; the operator has an Apple Developer account. The release pipeline never exposes the signing identity to forks (§5.1).
4. **Adoption order:** DECIDED 2026-09-25 (operator): ccwho's mutant harness first, then liveapp (check.sh, the pty runner, the vitest gate, the reap backend), then a **standard install for the liveapp fleet** (the container image and `liveapp init` provision it).
5. **Private macOS SPI:** DECIDED 2026-09-25 (operator): accepted, with the loud fallback and `doctor` report of §4.3.
6. **TCC default:** DECIDED 2026-09-25 (operator): accepted. By default a macOS job does not inherit the terminal's privacy permissions (strong tracking); the start warning names the fix; the grant survives upgrades via the signed bundle; `--inherit-terminal-permissions` is the escape hatch.
7. **Signing:** DECIDED 2026-09-25 (DevEx D12, reopened): Developer ID-signed and notarized `Sheepdog.app`; Homebrew as a cask.
8. **Command shape:** DECIDED 2026-09-25: `kill <pid>` and `strays` are in v1 (§3.0); `sheepdogd` is a later option. The original proposal: `sheepdog kill <pid>` kills the **proved** tree (live ppid descendants; on macOS also the `p_puniqueid` links) and **lists suspects** with their evidence (orphaned, same session or group, started after the root); `--include-suspects` kills them too. `sheepdog strays` lists your processes reparented to PID 1/launchd with size, age, command and best-known origin (it would have found the 67 GB process). A per-user recorder (`sheepdogd`, no root) that samples parent links, so that `kill` can prove more after the fact, is a later option.
9. **Name:** DECIDED 2026-09-25: `sheepdog` (was `deepkill`). It names both halves: `run` keeps the flock together, `strays` finds the ones that got away. Free on crates.io and Homebrew; npm is `@lukaso/sheepdog`. The orphan command is `strays` (was `orphans`). Rejected: `unfork` (collides with whitequark/unfork and the GitHub-fork meaning), `deepkill` (npm taken; reads oddly as a supervisor), `custody`.

## 9. Prior art

- **`timeout`, `tini -g`, `dumb-init`, `setsid`, the `process-wrap`/`command-group` crates:** group- or session-based, so `setsid` escapes.
- **Jenkins ProcessTreeKiller:** an env-tag sweep; misses Apple binaries, and `env -i` defeats it.
- **systemd scopes, cgroup v2:** complete, but Linux-only and host-dependent.
- **`catatonit`, `s6`:** PID-1 inits; they reap zombies but do not track a job.
- **Apple's own tools** use responsibility to attribute processes. No job-kill tool we found uses it.

## 10. Developer experience (DevEx review, 2026-09-25)

```
TARGET DEVELOPER PERSONA
========================
Who:       an AI coding agent: a Claude Code session, the liveapp loop, a ccwho run.
Context:   told by a CLAUDE.md rule or a harness to wrap hang-prone commands; has never seen the tool.
Tolerance: seconds. If sheepdog costs more than writing a `timeout` line, the agent writes the `timeout` line.
Expects:   `--help` that teaches the first command; exact, parseable output; errors that name the fix.
Secondary: the harness author (wires it in once; exit codes, --status-fd, Docker install);
           the developer after a crash (`strays`, `kill <pid>`).
```

### Developer perspective (confirmed by the operator)

"I'm a Claude Code session in ~/projects/ccwho-asks. CLAUDE.md says: 'wrap hang-prone test runs in sheepdog run'. I have never seen the tool. I run `sheepdog --help`. [UNKNOWN in r8: no help text, no example, no first flag.] I guess `sheepdog run --timeout 30s -- python3 -m unittest ...`. It works. A mutant hangs; after 30 s I get exit 124 and a one-line reason on stderr [wording UNKNOWN]. Did it kill everything? I'd need `--status-fd` and JSON; nothing in the stderr line tells me it exists. Later my harness sends `child.kill('SIGINT')` to the sheepdog pid; the root never gets it (a stated limit); nothing on screen says why. I escalate to SIGKILL on the pid; the tree leaks until someone runs `sheepdog sweep`, which I have never heard of. A macOS test that reads ~/Documents fails with 'Operation not permitted' and I blame the test."

### Competitive benchmark (estimates; nothing is built)

Clock: an agent that has never seen the tool is told to use it → its first wrapped run where a hung tree is killed **and** it can tell that it worked.

| Tool | Start → result | Time (est.) | DX choice | Kills escapees? |
|---|---|---|---|---|
| GNU `timeout` | known from training; Linux preinstalled, macOS via coreutils | ~0–1 min | one flag, exit 124 | no: group only (the 09-25 leak) |
| npm `tree-kill` (18.4 M downloads/week) | `npm i` + 3 lines | ~2 min | a function call | no: live ppid tree only (`pgrep -P`) |
| `systemd-run --user --scope` | one command | ~1 min where it exists | cgroup | yes; no macOS, no containers |
| process-wrap (Rust) | library integration | ~10 min | wrapper types | no: group/session |
| sheepdog r8 | install, read `--help`, run | ~3–5 min, help unspecified | `run -- cmd`, exit 124, `--status-fd` | yes |

**Target (operator, D5): Champion, under 2 min once installed.** The real competitor is the `timeout` one-liner an agent writes without thinking.

Sources: [tree-kill on npm (Socket.dev)](https://socket.dev/npm/package/tree-kill), [node-tree-kill](https://github.com/pkrumins/node-tree-kill), [process-wrap](https://github.com/watchexec/process-wrap).

### 10.4 The magical moment: the kill report (D6)

Whenever sheepdog kills anything, it prints a short report on stderr. This is where the agent sees that a process which would have leaked was caught, and where it learns the next command.

```
sheepdog: timeout 30s: ended the job, 4 processes killed, 1 of them had escaped:
  pid 4242  python3 -m unittest …   escaped with setsid, reparented to launchd
sheepdog: tree clean. Machine-readable: --status-fd.
```

- **"tree clean" is printed only when tracking was complete.** With the fallback (`--inherit-terminal-permissions`, or the SPI missing) the last line is `sheepdog: no members left that sheepdog could see (tracking degraded: <reason>).` `--quiet` does not suppress this line (outside voice F2).
- The report itself lists every escapee it killed; the job is gone afterwards, so there is no `ps` pointer (outside voice F4).

Other endings use the same shape:
- **Clean exit with leftovers:** `sheepdog: command exited 0; 2 leftover processes killed (pid 91 node, pid 93 esbuild).`
- **Kill deadline passed (exit 125):** `sheepdog: 1 process is still alive after 10s: pid 9 python3 (state D: waiting on disk I/O). The tree is NOT clean.`
- **Nothing to kill:** it prints nothing. A normal run adds no output.
- **`--quiet`** suppresses every `sheepdog:` line except errors. Machine callers read `--status-fd`.

### 10.5 The CLI surface (specified by this review)

**`sheepdog` with no arguments or `--help`:** the first screen teaches the first command. Examples come first; the flag reference follows.

```
sheepdog: run a command and make sure every process it starts is gone at the end,
including processes that escaped with setsid, double-forks or reparenting.

  sheepdog run --timeout 5m -- npm test        stop the whole tree after 5 minutes
  sheepdog run --max-mem 2G -- python3 job.py  stop it if the tree uses more than 2 GB
  sheepdog strays                              list leaked processes of yours, biggest first
  sheepdog kill 4242                           kill 4242 and the processes it provably started
                                               (see first: sheepdog ps 4242)

To stop a running job, send TERM to sheepdog. Exit 124 means a limit fired.
Commands: run, kill, strays, ps, sweep, doctor. `sheepdog help <command>` for details.
```

**Grammar:**
- `--timeout 30` is seconds, as in `timeout(1)`; units `s`, `m`, `h` are accepted. `--max-mem` takes `K`, `M`, `G`.
- A command that is not a sheepdog subcommand is a usage error that shows the fix: `sheepdog: 'python3' is not a sheepdog command. To run it under sheepdog: sheepdog run -- python3 job.py` (exit 2).
- **Usage errors exit 2** for every subcommand **except `run`**, which exits 125 as `timeout(1)` does, because `run` passes the command's own codes through and 2 could be the command's. `--status-fd`/`--json` carry `error.code` either way.
- Everything after `--` is the command, verbatim.

**Output modes** (the GitHub CLI pattern):
- On a tty: a table with color.
- Piped: the same columns, no color, no truncation.
- With `--json`: one JSON object per line.

Every JSON line and the `--status-fd` line carry `"v": 1` and the job id. Errors in JSON are `{"v":1,"error":{"code":"…","message":"…","fix":"…"}}`.

**`sheepdog --version`:** version, commit, platform, and whether the macOS responsibility API is active.

### 10.6 The agent snippet (in the README, copied into CLAUDE.md / AGENTS.md)

```markdown
## Commands that may hang or leak processes
Wrap them: `sheepdog run --timeout 5m -- <command>`. It kills the whole process tree when the command ends or a limit fires, including processes that escaped. Exit 124 means a limit fired; read the `sheepdog:` lines on stderr. To stop a job, send TERM to sheepdog (SIGINT to its pid does not reach the command). Leaked processes from earlier runs: `sheepdog strays`.
```

**README order:**
1. what sheepdog does, in two lines;
2. install (one command per platform);
3. the three examples from `--help`;
4. the agent snippet;
5. limits (§4.1, in plain words);
6. macOS privacy (§4.4);
7. reference.

### 10.7 Error paths (traced)

| Situation | r8 (before) | Specified now (problem + cause + fix) |
|---|---|---|
| `sheepdog python3 x` | unspecified | the usage error in §10.5 |
| INT sent to the sheepdog pid | silent (D4) | the 3 s hint (§3.1, D9) |
| a protected folder on macOS | the test fails with `Operation not permitted` | the start warning (§4.4, D10) |
| `sheepdog kill 1` | "refused" (§3.0) | `sheepdog: refusing to kill pid 1 (launchd): it is the system's init process. Nothing was signalled.` (exit 1) |
| `sheepdog strays --kill` without a filter | "refuses" (§3.0) | `sheepdog: --kill needs a filter (--min-mem, --older-than or --cmd), so that it never kills every stray at once. Run sheepdog strays first to see them.` (exit 2) |
| the responsibility API is missing | "loud warning" (§4.3) | `sheepdog: the macOS responsibility API is not available here, so tracking falls back to parent ids (fast-escaping processes can be missed). Details: sheepdog doctor.` |
| kill deadline | exit 125 | the deadline report in §10.4 |
| auto-sweep acted | unspecified | the one line in §3.5 |

Messages may link to the public README sections (the repo is public, §8 Q3).

### 10.8 Install and upgrade

**Install:**
- **macOS:** `brew install --cask lukaso/tap/sheepdog`, or `npm i -g @lukaso/sheepdog`, or `curl -fsSL https://github.com/lukaso/sheepdog/releases/latest/download/install.sh | sh` (installs `Sheepdog.app` to `~/Applications` and links the CLI into `~/.local/bin`; it verifies the checksum and the Developer ID signature). Each channel owns its own copy (D12, reopened).
- **Linux and Docker:** the static binary, pinned and verified:
  ```dockerfile
  ARG SHEEPDOG_VERSION=1.0.0
  RUN curl -fsSL -o /usr/local/bin/sheepdog \
        "https://github.com/lukaso/sheepdog/releases/download/v${SHEEPDOG_VERSION}/sheepdog-linux-$(uname -m)" \
   && echo "<sha256 from the release>  /usr/local/bin/sheepdog" | sha256sum -c - \
   && chmod +x /usr/local/bin/sheepdog
  ```
- The public URLs are valid: the repo is public (§8 Q3).

**Upgrade policy:**
- semver;
- **the stable contracts** are the subcommands and flags, the exit codes, the `--status-fd` and `--json` schemas (`"v": 1`; only additive changes inside v1) and the stderr line prefix `sheepdog:`;
- a deprecated flag warns one minor release before removal and names its replacement;
- a CHANGELOG entry for every release.

### 10.9 Measuring the target (D13)

**The agent first-use eval:**
- **Setup:** a fresh Claude Code session with no memory. It gets the installed binary and only the agent snippet.
- **Task:** "run `./hang.sh` with a 10 s limit". The fixture starts a `setsid` escapee.
- **Pass:** the escapee is dead (checked by the fixture's recorded identity), the agent's final message states what was killed, and the wall time is under 2 min.
- **When it runs:** in phase 3, and again whenever `--help`, the kill report or an error text changes. The operator runs it with a bounded token budget.
- **What it checks:** behavior, not wording.

### 10.10 Journey map (after this review)

| Stage | The agent does | Friction found | Status |
|---|---|---|---|
| Discover | reads the agent snippet in CLAUDE.md | no snippet existed | fixed (§10.6) |
| Install | one command; a harness or image provides it | a Homebrew formula or a bare binary loses macOS grants on upgrade | fixed (D12 reopened: signed bundle, cask) |
| First use | `sheepdog --help`, then `run --timeout` | no help text, no first example | fixed (§10.5) |
| Real use | wraps runs; reads the kill report | cannot tell a clean kill from a leak | fixed (D6, §10.4) |
| Debug | a SIGINT does nothing; a protected folder fails | silent traps | fixed (D9, D10) |
| Recover | its supervisor was SIGKILLed | a leak until someone runs `sweep` | fixed (D8) |
| Upgrade | `brew upgrade` / new release | the grant is lost; no schema version | fixed (D12, §10.8) |

### 10.11 First-time agent confusion report (0G), with dispositions

```
T+0:00  CLAUDE.md says "wrap hang-prone runs in sheepdog run". Runs `sheepdog --help`.   -> §10.5 first screen
T+0:20  Tries `sheepdog python3 -m unittest` (forgot `run --`).                         -> usage error with the fix
T+0:40  `sheepdog run --timeout 30 -- python3 -m unittest …`; a mutant hangs.
T+1:10  Exit 124 + kill report: "1 had escaped … tree clean".                          -> D6, the success moment
T+1:20  Harness later sends child.kill('SIGINT'); nothing happens.                      -> D9 hint after 3 s
T+1:30  Escalates to SIGKILL on the pid; next `run` sweeps the dead job.                -> D8 auto-sweep
T+?     A test under ~/Documents fails.                                                 -> D10 start warning
```

### 10.12 Not in scope (considered, deferred)

- **A `tree-kill`-compatible JS function in `@lukaso/sheepdog`.** tree-kill has 18.4 M downloads a week and misses every escapee. It is a new API, so it is a later decision.
- **A Claude Code hook that wraps commands automatically.** It is a new integration channel.
- **An OCI image for `COPY --from`.** It is a new channel; the static binary covers containers.
- **A docs site.** The README and `--help` carry v1.
- **Windows, and `sheepdogd`** (§8).

### 10.13 What already exists (reused)

- the exit codes of `timeout(1)` (124/125/126/127) and its bare-seconds grammar;
- the GitHub CLI's tty/pipe output rule;
- your ccwho rule for destructive commands (named targets act, computed targets ask or need `--yes`);
- liveapp's lesson that stderr is not a monitoring surface: machine callers read `--status-fd`, and humans and agents read the `sheepdog:` lines.

### 10.14 DX scorecard

```
+====================================================================+
|              DX PLAN REVIEW: SCORECARD (sheepdog r9)                |
+====================================================================+
| Dimension            | Before | After  |
|----------------------|--------|--------|
| Getting Started      |  3/10  |  8/10  |  help specified; one-command install; repo URL pending
| API/CLI              |  6/10  |  8/10  |  grammar, output modes, schema version
| Error Messages       |  3/10  |  8/10  |  8 paths traced; no doc links yet
| Documentation        |  1/10  |  7/10  |  README order + agent snippet; no site (deferred)
| Upgrade Path         |  3/10  |  8/10  |  semver, stable contracts, grant keyed to bundle ID (Developer ID rebuild: phase 0)
| Dev Environment      |  6/10  |  8/10  |  non-tty safe, Docker snippet, arm/x86
| Community            |  2/10  |  6/10  |  public repo decided; license and CONTRIBUTING still to write
| DX Measurement       |  1/10  |  7/10  |  agent first-use eval on text changes
+--------------------------------------------------------------------+
| TTHW (agent)         | unknown (no help) -> < 2 min target, measured by §10.9
| Competitive Rank     | Champion (target)                                   |
| Magical Moment       | designed, via the kill report on stderr             |
| Product Type         | CLI tool with a machine interface                   |
| Mode                 | DX POLISH                                           |
| Overall DX           |  3/10  |  7/10  |
+====================================================================+
| Zero Friction: covered  | Learn by Doing: covered (examples first)  |
| Fight Uncertainty: covered | Opinionated + Escape Hatches: covered  |
| Code in Context: covered (agent snippet, Docker snippet)             |
| Magical Moments: covered                                            |
+====================================================================+
```

Remaining DX debt: **Community (6/10)**: choose a license and write CONTRIBUTING before the first push.

### 10.15 DevEx decisions (2026-09-25)

| # | Decision | Answer |
|---|---|---|
| D1 | name | `sheepdog`; the orphan command is `strays` |
| D2 | design doc first | skipped: the plan is the design doc |
| D3 | primary persona | AI coding agent |
| D4 | empathy narrative | confirmed |
| D5 | time-to-first-success target | Champion, under 2 min once installed |
| D6 | magical moment | the kill report on stderr |
| D7 | review mode | DX POLISH |
| D8 | auto-sweep | on every `run` start, same owner, bounded |
| D9 | SIGINT limit | a one-line hint after 3 s |
| D10 | macOS privacy | a start-time warning with the exact fix |
| D11 | flag name | `--inherit-terminal-permissions` (the operator's wording) |
| D12 | macOS packaging | **reopened:** signed `Sheepdog.app` (bundle ID), Homebrew cask; replaces the stable-path design |
| D13 | measuring the target | an agent first-use eval in phase 3 and on text changes |
| D14 | kill scope | never wider than the named target; a supervisor pid ends its job |
| D15 | stable-path ownership | dropped: D12 reopened removed the shared file |
| TODO | tree-kill drop-in; Claude Code hook | in TODOS.md (GitHub issues when the repo is pushed) |

**Outside voice (Codex, gpt-6-astra, high effort):** 2 P1 + 5 P2, all checked against the plan text. F1 became D14. F5 became D15, which D12 (reopened) then dissolved. F2, F3, F4, F6 and F7 were fixed as follow-through of approved contracts (§3.0, §3.1 status, §3.5, §10.4, §10.5, cell 28).

## 11. Review record

**Round 1:** 11 findings (3 P1). **Round 2:** 10 findings (3 P1). **Round 3:** 7 findings (1 P1) plus P3s. **Round 4:** 5 P2 + 6 P3, **no P1**. **Round 5:** 5 P2 + 8 P3, **no P1**. **Round 6:** 2 P2 + P3s, **no P1**. **Round 7** (the no-setup modes and DevEx additions): 8 P2 + P3s, **no P1**. All P1s were re-measured by the planner before they were accepted.

| Round | # | Sev | Finding | Change |
|---|---|---|---|---|
| 1 | F1 | P1 | `puniq` alone loses ~1/3 in a fast double-fork | responsibility added (§2, §3.2) |
| 1 | F2 | P1 | ids repeat after a reboot | boot id in the journal (§3.5) |
| 1 | F3 | P1 | sid, pgid and tag match innocents | not membership; safety cells (§3.2, §6) |
| 1 | F4–F11 | P2/P3 | identity before signal, orphan reaping, checker, launchd leaks, scoped sweep, native npm bin, pidfd fallback, details | §3.1, §3.3, §3.5, §4.1, §6, §7 |
| 2 | A | P1 | responsibility disappears when the root exits, even as a zombie | the supervisor is the responsible process (SETEXEC + disclaim); sticky membership; snapshot before TERM; macOS `sweep` stated as journal + `puniq` only (§2, §3.1–3.5) |
| 2 | B | P1 | the phase-0 gate measured the state that cannot fail | the checker and the stress run after the root is reaped; the r2 design is the control (§6, §7) |
| 2 | C | P1 | disclaim removes the terminal's TCC grants | §4.4, `doctor`, `--inherit-terminal-permissions`, cell 19, question 6 |
| 2 | 1 | P2 | `puniq` resets to 1 on exec after reparenting | §2 corrected; `sweep` loss stated (§3.5) |
| 2 | 2 | P2 | the pid-reuse guard has no failing case; use the uniqueid SPI | R is a set of uniqueids; the guard is deleted (§3.2) |
| 2 | 3 | P2 | nested and re-disclaimed members are found only by a scan | registration through an inherited fd; cell 15 (§3.2) |
| 2 | 4 | P2 | the kill loop has no bound | kill deadline, exit 125; cell 20 (§3.3) |
| 2 | P3s | P3 | −1 semantics; rollback T-state on macOS; STOP fallback; journal write order; missing controls | §3.2, §3.3, §3.5, §6 |

| 3 | 1 | P1 | registration put any written uniqueid into R (158 processes on this Mac are responsible to iTerm2) | socket registration; the kernel names the peer; the peer must already be a member; ack before disclaim (§3.2); two safety cells |
| 3 | 2 | P2 | Node and Python close an inherited registration fd | env path + nonce, no inherited fd; cell 15 with Node and Python launchers |
| 3 | 3 | P2 | ctrl-Z froze enforcement while escapees ran | the supervisor stops and continues the tree; timeout counts running time; `kill` sends CONT (§3.1, §3.3); cell 21 |
| 3 | 4 | P2 | the "stdin is not a tty" INT rule is wrong both ways | forward only when not the foreground group (§3.1); cell 22 |
| 3 | 5 | P2 | a member that disclaims again loses its subtree when it exits; the stickiness mutant was unreachable | R grows from members (§3.2); cell 24; mutation list names the reachable cells |
| 3 | 6 | P2 | the root inherited Rust's SIGPIPE-ignore | the root gets the caller's signal state (§3.1); cell 23 |
| 3 | 7 | P2 | a safety cell tested a guard r3 had removed | cell deleted; where its invariant lives is stated (§6) |
| 3 | P3s | P3 | re-exec detection and path, lock order and purpose, exit-code precedence, cell 8 on Linux, controls for 17/19, three symbols named, TCC and code signature | §3.1, §4.3, §4.4, §6, question 7 |

| 4 | 1 | P2 | ctrl-Z: SIGSTOP interrupted the root's own TSTP handler (a pager left the terminal in raw mode) | group members stop by themselves, bounded wait; escapees are stopped at once (§3.1); cell 21(c) |
| 4 | 2 | P2 | registration was single-level; O → A → B lost B's subtree | the env carries the whole chain; register with every supervisor (§3.2); cell 15 three levels |
| 4 | 3 | P2 | a programmatic INT to the sheepdog pid is indistinguishable from the tty's | forward in the foreground only to escapees; the limit is stated, with the process-group workaround (§3.1); cell 22(c) |
| 4 | 4 | P2 | exit codes are ambiguous (125, 123 collisions) and a one-way door | `timeout(1)` codes plus `--status-fd` JSON; first trigger wins (§3.1); cell 26 |
| 4 | 5 | P2 | cells 21–24 were in no gate; registration mutants cannot go red on Linux | cells in phase 1; registration is macOS-only (§3.2, §7) |
| 4 | P3s | P3 | macOS peer identity (`LOCAL_PEERTOKEN`, after the record); membership check uses the scan's function; HUP on terminal close; socket path, permissions, length, no CLOEXEC_DEFAULT; TSTP during the kill loop, bare SIGSTOP, CONT-all mutant; cell 24 `puniq` = 1; the ack-miss window | §3.1, §3.2, §6; cells 21, 22, 24, 25; fd-passing safety cell |

| 5 | 1 | P2 | ctrl-Z in an orphaned group hangs forever (nobody sends CONT, and the timeout counts running time) | stop only when provably not orphaned (§3.1); cell 21(e) |
| 5 | 2, 3 | P2 | INT/HUP forwarding: undefined with no tty; a group signal in the background arrived twice; the advice contradicted itself | **class fix:** forward INT/HUP only to members outside the supervisor's group, in every state; one stated limit and one piece of advice (§3.1); cell 22 rewritten |
| 5 | 4 | P2 | TSTP work in the handler allocates; a self-pipe loses the kernel's discard of a pending stop | handler records only; the event loop does the work and coalesces (§3.1); cell 21(d) |
| 5 | 5 | P2 | the fd-passing and non-member safety cells could not go red; phase 1 named phase-2 safety cells | fd-passing cell deleted (where its invariant lives is stated); non-member decoy is another job's supervisor; "known set" defined; safety cells listed per phase (§3.2, §6, §7) |
| 5 | P3s | P3 | status fd CLOEXEC; exit-code order and status fields; cells 21(b)/(c) and 25 fixed so their mutants can fail; chain depth and budget; accept deadline; `mkdtemp` socket dir; bind failure; a TSTP-ignoring root is stopped (kept as deliberate) | §3.1, §3.2, §6 |

| 6 | 1 | P2 | the kernel sends terminal-close HUP to the session leader only; with sheepdog as session leader the root got none | session-leader exception: forward HUP to all (§3.1); cell 22′(g) |
| 6 | 2 | P2 | the parent-only orphan check fails for npm, make and `sh -c`, and races with a dying parent | the kernel decides: default TSTP + `raise` (§3.1); cells 21′(f), (g) |
| 6 | P3s | P3 | coalescing boundary; double delivery from callers that signal pid and group; pid-only callers who escalate to SIGKILL; the root's group not stated | counter read before the self-stop; limits stated in §3.1 and §4.1; `--forward-int-to-root`; cells 21′(h), 22′(h) |

**Trend:** rounds 4–6 found no P1. All of their P2s were in signal forwarding and job control. That area is inherently a set of stated trade-offs, not a closed proof. The next review is the DevEx review (below), then one more adversarial round over the whole plan.

| DevEx | — | — | one DevEx review (POLISH, persona: AI agent) + Codex outside voice | §10; 15 decisions; 2 TODOs |

**Round 7 folded in (r11).** Next: **phase 0**, the Rust spike. Reason: rounds 4–7 found no P1, and their P2s are contract details that the spike's cells now pin; more paper rounds open new surface faster than they close it. Previously planned: one more adversarial round over §3.0 (no-setup modes) and the DevEx additions (§3.5 auto-sweep, §3.1 hint, §4.4 bundle), then phase 0.

## GSTACK REVIEW REPORT

| Run | Status | Findings |
|---|---|---|
| plan-devex-review (DX POLISH, persona: AI coding agent) | done | 15 decisions (D1–D15), 8 error paths specified, the CLI surface specified, 2 TODOs; overall DX 3/10 → 7/10 |
| Outside voice: Codex (gpt-6-astra, high) | completed, issues found | 2 P1 + 5 P2; all 7 checked against the plan; F1 → D14, F5 → D15 (dissolved by D12 reopened), F2/F3/F4/F6/F7 fixed as follow-through |
| Verification done in-review | measured | TCC keying on this Mac: bare binary by path, bundle by bundle ID, ad-hoc re-sign revokes; formula = ad-hoc, cask keeps Developer ID |
| Adversarial rounds 1–6 (earlier today) | done | 7 P1 in rounds 1–3, none in rounds 4–6 (§11) |

**VERDICT:** DX plan is ready for one more adversarial round (the no-setup modes and the DevEx additions), then phase 0. The Community debt (4/10) is unblocked: the repo will be public. Phase 0 carries three TCC checks: a Developer ID rebuild keeps the grant; the grant holds through the PATH symlink; the bundle row shows in the Settings list or the `+` path is needed.

**CROSS-MODEL:** Codex agreed with the design direction and found contract gaps the in-host review missed (the kill-scope widening, the unqualified "tree clean"). There was no disagreement on the approved decisions.

All §8 questions were answered by the operator on 2026-09-25 (Q1–Q6; Q7–Q9 decided earlier).

NO UNRESOLVED DECISIONS
