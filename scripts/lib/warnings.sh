# Compiler warning lines in a test log (issue #10): test-all fails a leg whose log holds one, and a
# Linux build whose log holds one (tests/dist/t_warnings.sh). POSIX sh; sourced.
#
# A warning line starts `warning: ` or `warning[CODE]: `. Colour codes are allowed before `warning`
# and before the colon: rustc 1.98.1 with colour writes `ESC[1m ESC[33m warning ESC[0m ESC[1m :`.
# The count is of lines (rustc's "N warnings emitted" is one too); a leg fails on any.
WARN_ESC=$(printf '\033')
WARN_SGR="(${WARN_ESC}\[[0-9;]*m)*"
WARN_RE="^${WARN_SGR}warning${WARN_SGR}(\[[A-Za-z0-9_]+\])?${WARN_SGR}: "
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
