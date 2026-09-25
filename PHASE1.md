# sheepdog — phase 1 build plan

**Status:** DRAFT, 2026-09-25. Phase 0 is closed (PLAN.md §7.1: gate passed, 7 review rounds, the last clean). This file turns PLAN.md §7's phase-1 row into an ordered, test-first build. PLAN.md stays the spec; where this file and PLAN.md differ, PLAN.md wins and this file is wrong.

**Phase-1 scope (PLAN.md §7):** `run`, the kill on every exit, signals and job control, `kill <pid>` (proved members, `--dry-run`); cells 1–12, 16, 20–24, 27, 28 (proved part); the no-setup safety cells (sibling command, `| tee` partner, two concurrent jobs, another user, the caller's shell and ancestors, the wrong-freeze rollback, the pidfd `ENOSYS` path). **Plus** everything §7.1 carries: the four round-7 P3s, the phase-1 HUP cell (now `#[ignore]`d), the TERM grace, the pid-reuse seam, pidfd, Linux amd64 and PID 1, the grant through a PATH symlink.

**Not in phase 1** (phase 2): caps (`--timeout`, `--max-mem`, `--max-procs`), `--status-fd`, the journal and `sweep`, registration, `strays`, suspects in `kill <pid>`, `doctor`, `--inherit-terminal-permissions`, the kill report's full form (§10.4). Phase 1 prints only the minimal lines it needs (errors, the deadline report, the D9 hint).

---

## 1. Architecture change: one event loop per OS

Phase 0 grew three separate waits (the root wait, the kill loop, the relay loop) and each round of review found a window between them. Phase 1 replaces them with **one loop** that owns every event. Everything else is a state the loop is in.

**Blocked signals.** At start `setup_signals` blocks, **for each signal the caller left at default**: TERM, INT, HUP, TSTP, TTIN, TTOU, CONT, CHLD. A caller-ignored signal stays ignored and is not watched (PLAN.md §3.1). The root is spawned with the caller's mask (SETSIGMASK, as now). No handler is ever installed, so nothing is reset by exec and nothing can be lost between a check and a block (the round-6 lesson).

**The loop's inputs:**

| Input | macOS | Linux |
|---|---|---|
| a watched signal | kqueue `EVFILT_SIGNAL` (fires for a blocked signal: probed in round 7) + a `sigpending` check after registration | `signalfd` in `poll` (replaces `sigtimedwait`: one fd, no timeout needed for signals) |
| the root exits | kqueue `EVFILT_PROC NOTE_EXIT`, then a **blocking** `waitpid(root, 0)` (P3-F1); registration failing with ESRCH = the root is exiting → blocking wait | SIGCHLD via the signalfd → `waitpid(-1, WNOHANG)` loop (also reaps adopted orphans) |
| a scan is due | kqueue `EVFILT_TIMER` | `poll` timeout |
| any other kevent/poll failure | **never** a blocking wait that ignores TERM (P3-F2 companion): log once and fall back to polling every 50 ms | same |

**States:** `Running` → (root exits, or TERM) → `Ending` (§3.3 kill) → `Done`; `Running` ↔ `Stopped` (job control). A TERM while `Ending` changes nothing (PLAN.md §3.1). A pending TERM at start → `Done` before any spawn (P3-F2).

**One order for "TERM and root exit together" (P3-F4):** the loop drains **all** ready events of one wake before it acts, then decides with this fixed order: a watched TERM wins over the root's exit (the caller asked to end the job; the exit code is death by SIGTERM). Same on both OSes, one cell.

**Membership while running.** Phase 0 only scanned at the end. Phase 1 scans on a timer while `Running` (default 250 ms, the same cadence phase 2's caps need), keeping the sticky `known` set current, so that INT/HUP forwarding and job control know who the **escapees** are (members outside the supervisor's process group). The end-of-job kill starts from that set.

## 2. Build order (each step: failing cells first, then code, then its mutants; commit per step)

| Step | What | Cells (red first) | Mutants that must go red |
|---|---|---|---|
| **S1** | The event loop (kqueue / signalfd+poll), blocked signal set, the P3-F1/F2/F4 fixes, non-ESRCH fallback | P3-F1 latency (≥1,000 runs, every exit < 100 ms); P3-F2 pending TERM → root never execs (`/usr/bin/touch` root); P3-F3 re-exec cell with a first-action root; P3-F4 one order; all phase-0 cells still green | blocking wait after NOTE_EXIT removed; no pre-spawn TERM check; MW2 on the re-exec cell; order swapped on one OS |
| **S2** | Continuous membership scan + TERM **grace** (§3.3 step 1: TERM all, CONT, wait `--grace`, then freeze-kill) | a member that exits cleanly on TERM gets no SIGKILL (records its signal); `--grace 0` goes straight to KILL | no TERM phase; no CONT after TERM (a stopped member never sees TERM) |
| **S3** | Identity: pidfd on Linux (`pidfd_open` → re-read start time → `pidfd_send_signal`), `ENOSYS`/`EPERM` fallback; macOS uniqueid re-check; **wrong-freeze rollback** (SIGCONT only if not T before our STOP) | pid-reuse seam: a decoy with a reused pid gets zero signals (decoy counts its own signals); forced `ENOSYS` (seccomp profile in Docker) passes the same decoy cell; rollback: a pre-stopped decoy stays stopped, a running one gets CONT | no identity check; CONT always |
| **S4** | INT/HUP (PLAN.md §3.1): forward only to members **outside** the supervisor's group; session leader → HUP to all; D9 hint after 3 s | cell 22 (a)–(f) exact INT/HUP counts; cell 22′(g) session leader; the relay variant (no double INT); **the phase-1 HUP cell** un-ignored and green | forward to all; forward to none; no session-leader rule; relay forwards INT |
| **S5** | Job control (PLAN.md §3.1): coalesced TSTP; group members stop by themselves (bounded 1 s), escapees get SIGSTOP; default TSTP + `raise` (kernel decides orphaned); CONT resumes only what we stopped; TSTP ignored while `Ending` | cells 21 (a)–(e), 21′ (f)–(h) | default TSTP handling; CONT all; SIGSTOP the group at once; no coalescing; stop even when orphaned |
| **S6** | `kill <pid>` (PLAN.md §3.0): target checks, proved set (ppid closure; macOS + `puniq`), a supervisor pid → its job (macOS: processes responsible to its uniqueid; Linux: its subreaper tree), a member → its subtree + the job hint, `--dry-run`; exit codes 0/1/2/125 | cells 27, 28 (proved part), 29 (a)(b); safety: `kill 1`, `kill $$`, another user's pid refused with zero signals | no target checks; hand-over for members; no `puniq` closure |
| **S7** | The remaining escape cells 1–12 and 16 as written in PLAN.md §6, and the other no-setup safety cells | 1–12, 16; sibling command, `\| tee` partner, two concurrent jobs, another user (Docker), the caller's shell and ancestors | as named per cell in PLAN.md §6 |
| **S8** | Platform matrix | the whole suite on macOS arm64, Linux Alpine + Debian **arm64 and amd64**, Linux as **PID 1** and under `--init`; the grant through a PATH symlink (operator toggle, as in phase 0) | — |

**Exit rules, every step:** the suite is green on macOS and on Alpine and Debian; every new cell was seen red first; every mutant was built, ran, and went red for its stated reason; no test acts on a fixed sleep (readiness only: the phase-0 first-launch lesson); cleanup kills only by recorded identity or marker. A review runs after S1, after S4, and after S8 (the phase-1 review), each until a round is clean.

## 3. Risks named now

- **signalfd and the relay.** The relay (Linux, "children sheepdog did not create") keeps its own small loop; S1 moves it onto signalfd too, and its cells (death by signal, latency, PDEATHSIG, mask, HUP) must stay green.
- **kqueue EVFILT_SIGNAL semantics** were probed once (round 7). S1 adds a cell that sends TERM twice and INT once while blocked and asserts each is seen.
- **The continuous scan's cost** (macOS: one `proc_listallpids` plus a responsibility call per same-uid process; round 1 measured ~0.3 ms per full scan at ~1,100 processes). S2 asserts a 250 ms cadence costs under 1 % CPU on this Mac.
- **Job control is the least-tested area** (six review rounds found only paper findings there). S5 gets its own review before S6.
