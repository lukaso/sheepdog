#!/bin/sh
# The README's shell blocks are pasted into a terminal, and macOS's default shell, an interactive
# zsh, does not read `#` as a comment there (interactivecomments is off): `rm -rf x  # a note`
# also removes `#`, `a` and `note`. So no pasteable line may carry a comment: a `#` at the start,
# or after a space or one of ; & | ( ), outside quotes, a command substitution and a markdown link
# (inline code: only after a command word, since a span that starts with # names the character).
# Premise row: an interactive `zsh -f` passes the `#` on (skipped where there is no zsh).
# Controls: the checker refuses a comment in every form a command can take (inline code, an
# indented fence, ~~~, console, an info string, an indented block, a longer fence, after an
# escaped quote, after ; or &&, after a subshell's ")") and accepts a `#` inside quotes or a word,
# ${#a}, $#, $(a)#b, a Dockerfile comment, plain inline code, inline code that starts with #, and a
# markdown link in an indented line.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
chk() { # file -> the offending lines on stdout; exit 1 if any
  python3 - "$1" <<'PY'
import re, sys
PASTE = {"", "sh", "bash", "shell", "zsh", "console"}   # what a reader pastes into a shell
def blank(l):   # quoted text, a command substitution (its ")" is no subshell's) and a markdown link
    t = re.sub(r"'[^']*'|\"(?:[^\"\\]|\\.)*\"", "''", l)
    t = re.sub(r"\$\([^()]*\)", "x", t)
    return re.sub(r"\[[^\]]*\]\([^)]*\)", "x", t)
def comment(l):   # a `#` at the start, or after a space or one of ; & | ( ), outside those
    t = blank(l)
    return re.search(r"(^|[\s;&|()])#", t) is not None   # after a space, ; & | ( ) or at the start
bad = []; fence = None; lang = ""; prev_blank = True; indented = False
for n, l in enumerate(open(sys.argv[1]), 1):
    l = l.rstrip("\n")
    if fence is None:
        m = re.match(r"^\s*(`{3,}|~{3,})(.*)$", l)
        if m:
            fence = m.group(1); lang = (m.group(2).strip().split() or [""])[0]; prev_blank = False; continue
    else:
        m = re.match(r"^\s*(`{3,}|~{3,})\s*$", l)
        if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence):
            fence = None; continue
        if lang in PASTE and comment(l.strip()): bad.append("%d: %s" % (n, l))
        continue
    # an indented code block: four spaces or a tab, after a blank line or another such line
    if l.strip() and (prev_blank or indented) and re.match(r"^( {4,}|\t)", l) and not re.match(r"^\s*([-*+]|\d+[.)])\s", l):
        indented = True
        if comment(l.strip()): bad.append("%d: %s" % (n, l))
    else:
        indented = indented and not l.strip()
        for span in re.findall(r"`([^`]+)`", l):   # inline code, copied from prose: a comment after a
            t = blank(span)   # command word (a span that starts
            if re.search(r"(\S\s+|[;&|()]\s*)#", t): bad.append("%d: `%s`" % (n, span))   # with # names the character)
    prev_blank = not l.strip()
print("\n".join(bad)); sys.exit(1 if bad else 0)
PY
}
if command -v zsh > /dev/null 2>&1; then
  # an interactive zsh, detached from any terminal (its own session, no job control, no line
  # editor), so it reads the pipe and never takes over the terminal the cell runs in
  cat > "$FX/premise.sh" <<'SH'
h=$1; shift   # then zsh's own options, each a word
printf 'echo a # b\nexit\n' | env -i PATH=/usr/bin:/bin HOME="$h" perl -MPOSIX -e 'POSIX::setsid() != -1 or die "setsid\n"; exec @ARGV' zsh -f -i +m +Z "$@" 2>/dev/null | tr -d '\r'
SH
  out=$(sh "$FX/premise.sh" "$FX" | grep -x 'a # b')
  [ "$out" = "a # b" ] && pass "premise: an interactive zsh passes '#' on as an argument" || fail "premise: an interactive zsh read '#' as a comment ('$out')"
  # (the zsh must have run: an exact `a` line, the comment dropped; a zsh that refused the option
  # prints neither)
  sh "$FX/premise.sh" "$FX" -o interactivecomments > "$FX/ic.out"
  [ "$(grep -cx 'a' "$FX/ic.out")" = 1 ] && [ "$(grep -cx 'a # b' "$FX/ic.out")" = 0 ] \
    && pass "control: with interactivecomments on, the same zsh reads '#' as a comment" || fail "control: interactivecomments: $(tr '\n' '|' < "$FX/ic.out")"
  # the premise run under a pseudo-terminal whose input stays open ends at once (it used to take the
  # terminal over and wait for typing)
  if command -v script > /dev/null 2>&1; then
    # input that stays open: a pipe from a sleep (script refuses a FIFO), stopped by its pid once
    # script ends, so the pipeline does not wait for it
    s0=$(date +%s)
    (sleep 20 & echo $! > "$FX/sp"; wait) | { timeout -k 2 10 script -q /dev/null sh "$FX/premise.sh" "$FX" > "$FX/pty.out" 2>&1; echo $? > "$FX/pty.rc"; kill "$(cat "$FX/sp")" 2>/dev/null; }
    r=$(cat "$FX/pty.rc"); dt=$(( $(date +%s) - s0 ))
    [ $r = 0 ] && [ $dt -lt 8 ] && tr -d '\r' < "$FX/pty.out" | grep -qx 'a # b' && pass "the premise under a pty with open input ends in ${dt}s" || fail "the premise under a pty: rc=$r after ${dt}s"
  fi
fi
printf '```sh\nrm -rf x  # a note\n```\n' > "$FX/t1.md"; chk "$FX/t1.md" > /dev/null && fail "control: a trailing comment passed" || pass "control: a trailing comment is refused"
printf '```sh\n# a note\nls\n```\n' > "$FX/t2.md"; chk "$FX/t2.md" > /dev/null && fail "control: a comment line passed" || pass "control: a comment line is refused"
printf '```sh\necho '"'"'a # b'"'"' "c #d" e#f\n```\n```markdown\n## a heading\n```\n' > "$FX/t3.md"
# each form a pasteable command can take, with a comment: all refused
n=0
for form in '- remove it: `rm -rf x # a note`' \
  '- a step:\n\n  ```sh\n  rm -rf x # a note\n  ```' \
  '~~~sh\nrm -rf x # a note\n~~~' \
  '```console\nrm -rf x # a note\n```' \
  '```sh title=x\nrm -rf x # a note\n```' \
  'Text.\n\n    rm -rf x # a note\n' \
  '````sh\nrm -rf x # a note\n````' \
  '```sh\necho "a\\\\" # note "b"\n```' \
  '```sh\na;# note\n```' \
  '- then: `a && b;# note`' \
  '```sh\n(cd x)# note\n```'; do
  n=$((n + 1)); printf '%b\n' "$form" > "$FX/f$n.md"
  chk "$FX/f$n.md" > /dev/null && fail "control: a comment in form $n passed: $(head -3 "$FX/f$n.md" | tr '\n' '|')" || pass "control: a comment in form $n is refused"
done
printf '```dockerfile\n# a Dockerfile comment\nRUN true\n```\nSee `sheepdog help`, `#!/bin/sh`, `#[cfg(test)]` and `#`.\n```sh\necho ${#a} $# $(echo a)#b\n```\nAnd `echo ${#a} $# $(echo a)#b`.\n\n- a list item\n\n    see [npm](#npm)\n' > "$FX/t4.md"
chk "$FX/t4.md" > /dev/null && pass "control: a Dockerfile comment, plain inline code, inline code that starts with #, \${#a}, \$#, \$(a)#b and a markdown link in an indented line pass" || fail "control: a Dockerfile block or plain inline code refused: $(chk "$FX/t4.md")"
chk "$FX/t3.md" > /dev/null && pass "control: a # inside quotes or a word, or in a markdown block, passes" || fail "control: a quoted #, a word's # or a markdown block refused: $(chk "$FX/t3.md")"
if bad=$(chk "$SD_ROOT/README.md"); then pass "no comment in any README shell block"; else fail "README shell lines a pasted zsh would mangle: $(printf '%s' "$bad" | tr '\n' '|')"; fi
finish
