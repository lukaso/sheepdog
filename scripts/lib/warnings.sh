# Compiler warning lines in a test log (issue #10): test-all fails a leg whose log holds one, and a
# Linux build whose log holds one (tests/dist/t_warnings.sh). POSIX sh; sourced. Sourcing it also
# exports cargo's output lock (below) for every cargo run after it.
#
# A warning line starts `warning: ` or `warning[CODE]: `. Colour codes are allowed before `warning`
# and before the colon: rustc 1.98.1 with colour writes `ESC[1m ESC[33m warning[CODE] ESC[0m ESC[1m :`.
# The count is of lines (rustc's "N warnings emitted" is one too); a leg fails on any.
WARN_ESC=$(printf '\033')
WARN_SGR="(${WARN_ESC}\[[0-9;]*m)*"
WARN_RE="^${WARN_SGR}warning(\[[A-Za-z0-9_]+\])?${WARN_SGR}: "
# cargo's output lock, exported here, so whatever sources this file builds under it: test-all (the
# builds on this Mac) and, inside the containers, the Linux suites and build_alpine and
# build_amd64. Cargo then reports with no colour codes, no progress bar and every warning shown,
# whatever a cargo config file says (the environment beats a config file): a progress bar drawn
# into a log puts the next warning in the middle of a line, where no `^` match sees it, and
# `build.warnings = "allow"` hides every warning. The lock pins how cargo reports, never what it
# builds: warnings silenced by rustflags (`-A warnings`, `--cap-lints`), in the source
# (`#![allow(...)]`) or in Cargo.toml's `[lints]` are not covered (each changes what is compiled).
WARN_CARGO_ENV="CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never CARGO_BUILD_WARNINGS=warn"
for _warn_kv in $WARN_CARGO_ENV; do export "$_warn_kv"; done
unset _warn_kv
# warn_lock: print the lock as this shell exports it ("cargo output lock: NAME=VALUE ...", or
# "NAME unset"), and fail unless every name holds the lock's value. Each container build runs it
# right after sourcing this file: without the source line it is not found, and the build stops.
# Not caught: a change that removes both lines from an entry point (test-all does not check that a
# container log holds the lock line), or one that overrides a lock variable after warn_lock has
# run (it reads the environment where it runs).
warn_lock() (
  ok=0 out=""
  for kv in $WARN_CARGO_ENV; do
    n=${kv%%=*}
    if v=$(printenv "$n"); then out="$out $n=$v"; else v=""; out="$out $n unset"; fi
    [ "$n=$v" = "$kv" ] || ok=1
  done
  echo "cargo output lock:$out"
  return $ok
)
# warn_count LOG: the number of warning lines in LOG, read binary-safe, or `unreadable` when grep
# cannot read it
warn_count() (
  c=$(grep -a -c -E "$WARN_RE" "$1" 2>/dev/null)
  case $? in 0 | 1) echo "$c" ;; *) echo unreadable ;; esac
)
# warn_fail LOG: nothing when LOG holds no warning line, else why the leg fails (a log the check
# cannot read fails it too)
warn_fail() (
  w=$(warn_count "$1")
  case $w in
    0) ;;
    unreadable) echo "the log cannot be read for the compiler-warning check; $1" ;;
    *) echo "$w compiler warning line(s); $1" ;;
  esac
)
