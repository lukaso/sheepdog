# S2 fix round 1 (review of e59dd3a)

Two lenses: membership and the kill (A), can the tests fail (B). Every fix starts with a cell
that is red on e59dd3a; every fix ends with a mutant of the new code that turns that cell red.

## Fixes

| # | Finding | Red cell first | Fix | Mutant |
|---|---|---|---|---|
| 1 | A-P2-1 (macOS) a self-responsible caller's background job is killed: it is responsible to sheepdog, which kept the caller's uniqueid across `exec` | f1: a caller started with the disclaim runs `bg & exec sheepdog run -- true`; the job survives (and the plain-shell control) | the macOS relay: with children at start, fork a fresh supervisor (new uniqueid). The relay forwards TERM always, HUP only as session leader, never INT, and dies like the supervisor. The supervisor treats the relay's death as TERM (kqueue NOTE_EXIT; macOS has no PDEATHSIG) | no relay on macOS |
| 2 | A-P2-2 (macOS) a root that exits before the first scan loses its disclaimed children (`puniq` = root, never in `ever`) | f2: the root spawns `/bin/sleep M` with the disclaim and exits at once; 0 survivors in 20 runs | spawn the root with `POSIX_SPAWN_START_SUSPENDED`, read its uniqueid into `ever`, then SIGCONT | root identity read after it runs |
| 3 | A-P2-3 R growth has a race: GG escapes if C dies within one tick | f3 kept as a stated-limit cell (`#[ignore]`, runs on demand, measures the window) | amend PLAN §3.2: caught if a scan sees it while the member lives (window one tick) | n/a (a limit, not a fix) |
| 4 | B-P1-1 the TERM cell cannot see a SIGKILL during the grace | the handler writes `TERM <pid>`, sleeps 300 ms, writes `EXIT <pid>`; the cell asserts both, pid = the escapee | test change only | `grace_end = now` goes red |
| 5 | B-P2-1 nothing asserts the grace ends early | default grace, a member that exits on TERM: sheepdog ends in < 1 s (after a warm-up) | test only | drop the early break; drop the rescan |
| 6 | B-P2-2 TERM re-sent every grace pass is not caught | fixture `term-counter` (counts TERMs, stays alive); `--grace 500ms`; exactly one TERM line, member dead | test only | TERM every pass |
| 7 | B-P2-3 the deadline is extended by the grace, untested | deadline seam 300 ms, `--grace 500ms`, a TERM-ignoring member: exit 0, member dead | test only | drop `deadline + grace` |
| 8 | B-P2-4 the macOS fixture fails silently (no disclaim, no spawn) | `disclaim_reexec` exits non-zero on a missing symbol or a failed spawn, and the root propagates it; D and GG record pid + identity only after checking they are responsible for themselves (D) / not the caller's; cells assert one record | fixture | fixture skips the disclaim: cells red |
| 9 | B-P2-5 the scan-cost cell has 25 % margin and one job sample | min of 3 job samples | test only | 10 ms scan still red |
| 10 | B-P2-6 (Linux) no cell needs tracking while running | `--mode none`, fixture `escape-late` (C lives 600 ms after forking G, then exits so G goes to init): 0 survivors | test only | drop `tracker.refresh` |
| 11 | B-P3-1 the TERM-registration cell does not show the seam took effect | capture stderr, assert "cannot watch TERM"; fix the 40 % comment | test only | rename the seam: red |
| 12 | B-P3-2 `--leave-strays` + TERM: spec and code disagree | TERM to sheepdog with `--leave-strays` kills the stray | decision: TERM always kills the whole job; `--leave-strays` applies when the command exits by itself. PLAN §3.3 amended | drop `&& code.is_some()` |
| 13 | B-P3-3 the stop wait in the fixture has no bound | bound it (2 s), exit 3 | fixture | n/a |
| 14 | B-P3-4 C inherits the TERM handler | covered by #4 (the line's pid must be G's) | test | n/a |
| 15 | B-P3-5 `redisclaim` GG execs after a fixed 50 ms | GG waits until its parent has changed (bounded) | fixture | n/a |
| 16 | A-P3 `--grace inf` panics | `--grace inf` / `1e30`: exit 125, usage line, no panic text | `parse_duration` rejects non-finite and > 1 day | accept inf |

## Rejected (offered to the operator, not filed)

- A-P3 `--mode root-disclaim` ignores `--grace` / `--leave-strays`: it is the phase-0 control
  mode kept for the comparison cells, not a user mode.
