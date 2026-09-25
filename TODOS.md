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
