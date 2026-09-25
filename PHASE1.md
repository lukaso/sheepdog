# sheepdog — phase 1 build plan

**Status:** r2, 2026-09-25. Plan review round 1: no P1, 9 P2 + 13 P3, all applied (§5). Phase 0 is closed (PLAN.md §7.1). This file turns PLAN.md §7's phase-1 row into an ordered, test-first build. PLAN.md is the spec: where they differ, PLAN.md wins, **except** where this file names a PLAN.md amendment; those amendments are made in the same commit as the step that needs them.

**Phase-1 scope (PLAN.md §7):** `run`, the kill on every exit, signals and job control, `kill <pid>` (proved members, `--dry-run`); cells 1–12, 16, 20–24, 27, 28 (proved part); the no-setup safety cells. **Plus** everything §7.1 carries: the four round-7 P3s, the phase-1 HUP cell (now `#[ignore]`d), the TERM grace, the pid-reuse seam, pidfd, Linux amd64 and PID 1, the grant through a PATH symlink.

**Placed in phase 1 here** (PLAN.md did not place them): `--forward-int-to-root` (S4), `--quiet` (S4, suppresses the D9 hint), `--leave-strays` (S2, the opt-out of the kill on every exit), "a supervisor in the kill set is ended first" (S6).

**Not in phase 1** (phase 2): caps (`--timeout`, `--max-mem`, `--max-procs`), `--status-fd`, the journal, job ids and `sweep`, registration, `strays`, suspects in `kill <pid>`, the `kill <pid>` hand-over and cells 29–31 (PLAN.md §7 puts them in phase 2), `doctor`, `--inherit-terminal-permissions`, the full kill report (§10.4). Phase 1 prints only errors, the deadline report and the D9 hint. The "belongs to running job" hint names the **supervisor pid** until job ids exist.

---

## 1. Architecture: one event loop per OS

Phase 0 grew three separate waits (the root wait, the kill loop, the relay loop), and each review round found a window between them. Phase 1 replaces them with **one loop** that owns every event; everything else is a state of that loop.

### 1.1 Blocked signals, consumed synchronously

- **At start** sheepdog blocks the signals it handles **and** that the caller left at default. A caller-ignored signal stays ignored and unwatched (PLAN.md §3.1). **SIGCHLD is the exception:** it is always reset to default and blocked (review round 2, P1-B), whatever the caller did.
- **Which signals, when:** each step blocks only what it handles, so no phase-0 cell breaks in between. S1: TERM, CHLD (as phase 0). S4 adds INT, HUP. S5 adds TSTP, TTIN, TTOU, CONT.
- **No handler is ever installed.** Nothing is reset by exec, and nothing is lost between a check and a block (round-6 lesson).
- **Every signal is consumed exactly once:**
  - **Linux:** a `signalfd` (`SFD_CLOEXEC | SFD_NONBLOCK`, created **after** the relay fork) is read until empty.
  - **macOS:** kqueue `EVFILT_SIGNAL` reports a delivery attempt but does **not** consume the pending signal. So after each report, sheepdog takes it with `sigwait`, which returns at once because it is pending. At registration, one `sigpending` pass consumes anything that was already pending, also with `sigwait`. Otherwise a blocked INT stays pending forever and is forwarded again on every wake (plan review, P2-9).
- **The root** is spawned with the caller's mask (SETSIGMASK), as in phase 0.

### 1.2 The loop's inputs

| Input | macOS | Linux |
|---|---|---|
| a watched signal | kqueue `EVFILT_SIGNAL`, then `sigwait` to consume | `signalfd` in `poll` |
| the root exits | kqueue `EVFILT_PROC NOTE_EXIT`, then a **blocking** `waitpid(root, 0)` (P3-F1). If registration fails with ESRCH, the root is exiting: blocking wait. | SIGCHLD via the signalfd, then a `waitpid(-1, WNOHANG)` loop (it also reaps adopted orphans) |
| a scan is due | kqueue `EVFILT_TIMER` (250 ms) | `poll` timeout |
| any other kevent or poll failure | never a blocking wait that ignores TERM: log once, then poll every 50 ms (a blocked TERM stays pending, so polling loses nothing) | same |

### 1.3 States and decisions

- **States:** `Running`, then (root exit, TERM, or an INT/HUP that ends the job) `Ending` (§3.3 kill), then `Done`. `Running` and `Stopped` alternate under job control. A pending TERM at start goes straight to `Done` **before any spawn** (P3-F2).
- **One order when events coincide (P3-F4):** the loop drains **all** ready events of one wake, then decides in this fixed order, the same on both OSes: TERM, then the root's exit, then INT/HUP, then job control, then the scan timer. A TERM in `Ending` changes nothing.
- **Death by signal is preserved** (plan review, P2-3). If the supervisor ended the job because of TERM, it dies of SIGTERM. If it received INT or HUP and the root then died of that **same** signal, it dies of that signal after the kill. So a shell loop `while …; do sheepdog run …; done` stops on ctrl-C, as bash's wait-and-cooperative-exit expects. The relay copies whichever way the supervisor ended. Exit 143 is used only where `raise` cannot kill: as PID 1, where the kernel ignores signals with default actions. **PLAN.md amendment (S1):** §3.1's "124 … 143" line says "death by SIGTERM (exit status 143 only where the process cannot die of a signal, e.g. as PID 1)".
- **Fresh membership at every signal event** (plan review, P2-8). Forwarding (S4) and job control (S5) never act on the last timed scan alone. At the event, sheepdog rescans, acts, and repeats the §3.3 "close" step (scan, act on new members) until the set is stable.
- **Membership while running:** a timed scan (250 ms) keeps the sticky `known` set current between events, for the end-of-job kill and for phase 2's caps. The facts are those of PLAN.md §3.2: macOS responsibility **and** `puniq`, with R growing from members that became responsible for themselves; Linux subreaper descendants.

### 1.4 Job control (S5)

**PLAN.md amendment (S5):** §3.1's "the signal handler increments a counter and writes a self-pipe byte" becomes "TSTP/TTIN/TTOU are blocked and consumed by the loop".

The sequence, with probe results from the plan review:
1. The loop consumes TSTP, TTIN or TTOU (coalesced: all stop signals of one wake are one stop).
2. It does the bookkeeping: group members stop by themselves (bounded wait of 1 s, then SIGSTOP for the rest); escapees get SIGSTOP at once, after a fresh rescan. It records who was already in T.
3. It unblocks **only that signal**, `raise`s it, and re-blocks it once it resumes. Probed on both OSes: an unblocked pending TSTP or TTIN stops the process when the group is not orphaned, and the kernel discards it when the group is orphaned. That is the kernel's orphan decision, with no guess by sheepdog.
4. A blocked CONT still continues a stopped supervisor (probed on both OSes). The pending CONT is then consumed, and the loop continues only the members it stopped.

TTIN and TTOU are handled exactly like TSTP, and the self-stop raises the same signal, so the shell shows "Stopped (tty input)" (plan review, P2-2). A stop signal that arrives in `Ending` is consumed and ignored.

## 2. The pty harness (a prerequisite of S4, built first in S4)

Cells 21, 21′, 22(a)/(d) and 22′(g) need a real controlling terminal. The harness is test code in `tests/pty.rs`, with **its own control cell**:
- `openpty`; the child calls `setsid`, `TIOCSCTTY` and `tcsetpgrp`;
- a small fixture "shell" (`sd-fixture shell`) puts the job in its own process group, makes it the foreground group, and waits with `WUNTRACED`. It reports "stopped" or "exited", so a cell can observe that "the prompt returns";
- ctrl-C and ctrl-Z are written to the master as `\x03` and `\x1a`;
- everything it starts is recorded by pid and identity and killed only by those; the master is closed in a drop guard;
- **it never uses `script`** (cell 10 uses `script`, and the liveapp pty runner leaked).

Control cell: `sleep` under the fixture shell, ctrl-Z → "stopped", ctrl-C → "exited by INT". No sheepdog.

## 3. Build order (each step: failing cells first, then code, then its mutants; one commit per step)

| Step | What | Cells (red first) | Mutants that must go red |
|---|---|---|---|
| **S1** | The event loop (kqueue / signalfd + poll), consuming signals, the P3-F1/F2/F4 fixes, the non-ESRCH fallback, death by SIGTERM; the relay moves onto signalfd | P3-F1: exit latency < 100 ms in every one of ≥1,000 runs; P3-F2: a pending TERM means the root never execs (`/usr/bin/touch` root); P3-F3: re-exec cell with a first-action root; P3-F4: a debug seam delivers TERM and the root's exit in one wake, then the fixed order; a TERM sent twice while blocked is seen; the root has no signalfd (`/proc/self/fd`); all phase-0 cells green; **cell-3 stress ≥1,000 on each OS** | the blocking wait after NOTE_EXIT removed; no pre-spawn TERM check; MW2 on the re-exec cell; the order swapped on one OS; signalfd without CLOEXEC |
| **S2** | Timed membership scan; macOS `puniq` fact and R growing from members (§3.2); TERM **grace** (§3.3 step 1); `--leave-strays` | **cell 24 full** (a member that re-disclaims after it was observed); a member that exits cleanly on TERM gets no SIGKILL; a **pre-stopped** member records its TERM (the CONT after TERM is load-bearing); `--grace 0` goes straight to KILL; `--leave-strays` leaves a stray and exits; scan cost < 1 % CPU at 250 ms on this Mac | no `puniq` fact; no R growth; no TERM phase; no CONT after TERM |
| **S3** | Identity: pidfd on Linux (`pidfd_open`, re-read start time, `pidfd_send_signal`), `ENOSYS`/`EPERM` fallback; macOS uniqueid re-check; wrong-freeze rollback (SIGCONT only if not T before our STOP) | pid-reuse seam: a decoy with a reused pid gets zero signals (it counts its own signals); forced `ENOSYS` passes the same decoy cell; rollback: a pre-stopped decoy stays stopped, a running one gets CONT | no identity check; CONT always |
| **S4** | The pty harness (§2); INT/HUP (§3.1): forward only to members **outside** the supervisor's group, session leader → HUP to all, fresh rescan at the event; D9 hint after 3 s; `--quiet`; `--forward-int-to-root`; death by INT/HUP (§1.3) | pty control cell; cell 22 (a)–(f) exact INT/HUP counts; 22′(g) session leader; the relay variant (no double INT); two INTs → exactly two forwards (macOS consumption); **the phase-1 HUP cell** un-ignored; the relay HUP cell **rewritten** ("the leader dies of HUP" becomes "the tree is gone, and the relay dies of HUP"), bounded; a shell loop in the pty stops on one ctrl-C | forward to all; forward to none; no session-leader rule; the relay forwards INT; no macOS `sigwait` consumption (repeat forwarding); exit code instead of death by INT |
| **S5** | Job control (§1.4) | cells 21 (a) with a **breeding** escapee, (b)–(d); 21(e) rewritten: an orphaned group plus TSTP, the job keeps running and ends when the root exits (a 1 s root), not a hang; 21′(f); 21′(g) rewritten: the parent is killed while a stop is pending, and the job ends when the root exits; 21′(h); TTIN: `sheepdog run -- cat &` in the pty → "Stopped (tty input)" and escapees stopped | default TSTP handling; CONT all; SIGSTOP the group at once; no coalescing; stop even when orphaned; TTIN not handled |
| **S6** | `kill <pid>` (PLAN.md §3.0), proved part: target checks, the proved set (ppid closure; macOS + `puniq`), `--dry-run`, exit codes 0 / 1 / 2 / 125; **a supervisor in the kill set is ended first** (TERM, then wait up to its kill deadline, then the freeze). A supervisor is recognised by its **executable**: `proc_pidpath` / `/proc/<pid>/exe` is a sheepdog binary (stated limit: a renamed copy is not recognised). A member of a running job: its subtree only, and the output names the supervisor pid | cells 27, 28 (proved part), 29(f) (an inner supervisor in the tree: 0 inner escapees); safety: `kill 1`, `kill $$`, another user's pid refused with zero signals | no target checks; no `puniq` closure; freeze at once (inner escapees survive) |
| **S7** | The remaining escape cells 1–12 and 16 as written in PLAN.md §6; the other no-setup safety cells; cells 20 and 23 re-run as regressions (built in phase 0) | 1–12, 16; sibling command; `\| tee` partner; two concurrent jobs; the caller's shell and ancestors; "a same-uid process in the target's group, started before the target, **is not killed**" (the "not a suspect" form waits for phase 2's suspects) | as named per cell in PLAN.md §6 |
| **S8** | Platform matrix, one command `./test-all` | the suite on macOS arm64; Linux Alpine and Debian on arm64; **amd64 emulated** (same arm64 kernel, user space only: it does **not** test another kernel; separate image tags such as `sd-amd64-alpine`, never overwriting `rust:1-alpine`); **sheepdog as PID 1** (TERM exits 143 because `raise` is ignored for init; `raise(SIGTSTP)` is ignored, so the stop is treated as discarded) and under `--init`; the forced-`ENOSYS` leg (`docker run --security-opt seccomp=<profile with errnoRet 38 for pidfd_open>`); the another-user leg (a root container starts sheepdog as an unprivileged user through `setpriv`, and a decoy runs as another user); the grant through a PATH symlink (operator toggle, as in phase 0) | — |

**Reviews:** after S1, after S4, after S5 (job control is the least-tested area), and after S8 (the phase-1 review). Each continues until a round is clean.

**Exit rules, every step:**
- the suite is green on macOS and on Alpine and Debian;
- every new cell was seen red first;
- every mutant was built, ran, and went red for its stated reason;
- no test acts on a fixed sleep (readiness only: the phase-0 first-launch lesson);
- cleanup kills only by recorded identity or marker.

## 4. Risks and named residuals

- **The freeze (SIGSTOP) is not load-bearing in any safe test** (phase 0, mutant M3). It stays a named residual: only an unbounded fork storm separates it from repeat-alone, and that is not run on a workstation.
- **The relay** moves onto signalfd in S1. Its phase-0 cells (death by signal, latency, dead relay, mask, HUP) must stay green; the HUP cell is rewritten in S4.
- **kqueue `EVFILT_SIGNAL`** was probed in round 7 and again in the plan review. S1 adds a cell for it.
- **The scan cost** is bounded by an S2 cell.

## 5. Plan review round 1 (2026-09-25): 9 P2 + 13 P3, all applied

| # | Finding | Where it is now |
|---|---|---|
| P2-1 | a blocked TSTP cannot self-stop; PLAN says handler + self-pipe | §1.4 (consume, unblock only that signal, raise, re-block); PLAN amendment in S5 |
| P2-2 | TTIN/TTOU blocked without behaviour | §1.4; the TTIN cell in S5 |
| P2-3 | death by INT/HUP lost (shell loops ignore ctrl-C) | §1.3; S4 cells and mutant |
| P2-4 | S1 would break the relay HUP cell and hang the suite | §1.1 (INT/HUP blocked only from S4); the cell is rewritten, bounded, in S4 |
| P2-5 | cells 21(e) and 21′(g) use phase-2 `--timeout` | rewritten in S5 |
| P2-6 | cell list vs PLAN §7: 20, 23, 24; `--forward-int-to-root`; S6 vs 29; supervisor detection; supervisor-first | S2 (24 full), S7 (20, 23), S4, "Not in phase 1", S6 |
| P2-7 | no pty harness | §2, built first in S4, with a control cell |
| P2-8 | forwarding and stops act on a stale scan | §1.3 (fresh rescan + close at every event); 21(a) with a breeder |
| P2-9 | macOS never consumes a pending signal | §1.1 (`sigwait` after each report); a two-INT cell in S4 |
| P3s | signalfd flags and timing; CHLD wording; the one-wake order seam; review schedule; cell-3 stress after S1; freeze residual; pre-stopped member TERM cell; PID 1 outcomes; amd64 is emulated on the same kernel; the ENOSYS leg; the another-user leg; the pre-target decoy wording; 143 vs death by signal; `--leave-strays`/`--quiet`/job id placement | §1.1, §1.3, §3 (S1, S2, S4, S7, S8), §4, the scope lists |
