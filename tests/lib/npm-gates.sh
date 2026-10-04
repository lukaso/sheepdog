# The checks that stand between an npm install and its first run (tests/dist/t_npm_install.sh,
# PHASE3.md S5 cell 18); their own cells: tests/dist/t_npm_gates.sh. POSIX sh; sourced. Nothing
# here runs a file it judges, or any code of the package. Locals carry a prefix (the caller's
# variables are global too).

# runs_one_of PID LIST: the file PID is running (its txt entry in lsof, not its argv[0]), by real
# path, is one of LIST (one real path per line)
runs_one_of() {
  ro_f=$(/usr/sbin/lsof -a -p "$1" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)
  [ -n "$ro_f" ] || return 1
  ro_f=$(cd -P "$(dirname "$ro_f")" 2>/dev/null && pwd -P)/$(basename "$ro_f")
  printf '%s\n' "$2" | grep -qxF "$ro_f"
}

# npm_walk PATH: PATH with its links followed as the launcher follows $0 (at most 40)
npm_walk() {
  nw_me=$1 nw_n=0
  while [ -L "$nw_me" ]; do
    nw_n=$((nw_n + 1)); [ $nw_n -le 40 ] || break
    nw_l=$(readlink "$nw_me") || break
    case $nw_l in (/*) nw_me=$nw_l ;; (*) nw_me=$(dirname "$nw_me")/$nw_l ;; esac
  done
  printf '%s\n' "$nw_me"
}

# npm_target ARG0: the real path of the file npm/sheepr/bin/sheepr execs when it runs as ARG0
# (its resolution, copied: t_npm_gates.sh runs the launcher itself beside this copy on every layout
# it resolves); rc 1 if it resolves to nothing
npm_target() {
  nt_me=$(npm_walk "$1")
  nt_pkg=$(cd -P "$(dirname "$nt_me")/.." 2>/dev/null && pwd -P) || return 1
  for nt_d in "$nt_pkg/node_modules/sheepr-darwin-universal" "$nt_pkg/../sheepr-darwin-universal"; do
    if [ -x "$nt_d/Sheepr.app/Contents/MacOS/sheepr" ]; then
      nt_t=$(npm_walk "$nt_d/Sheepr.app/Contents/MacOS/sheepr")
      printf '%s/%s\n' "$(cd -P "$(dirname "$nt_t")" && pwd -P)" "$(basename "$nt_t")"; return 0
    fi
  done
  return 1
}

# npm_pnpm_shim TARGET NODE_PATH: the global shim pnpm 10.18.2 writes for a #!/bin/sh bin at
# "$basedir/TARGET" (measured 2026-10-02 with the pinned pnpm; t_npm_gates.sh's fixture is that
# measured text, and cell 18 puts the real one through npm_gate)
npm_pnpm_shim() {
  cat <<EOF
#!/bin/sh
basedir=\$(dirname "\$(echo "\$0" | sed -e 's,\\\\,/,g')")

case \`uname\` in
    *CYGWIN*|*MINGW*|*MSYS*)
        if command -v cygpath > /dev/null 2>&1; then
            basedir=\`cygpath -w "\$basedir"\`
        fi
    ;;
esac

if [ -z "\$NODE_PATH" ]; then
  export NODE_PATH="$2"
else
  export NODE_PATH="$2:\$NODE_PATH"
fi
if [ -x "\$basedir//bin/sh" ]; then
  exec "\$basedir//bin/sh"  "\$basedir/$1" "\$@"
else
  exec /bin/sh  "\$basedir/$1" "\$@"
fi
EOF
}

# npm_gate HOME INSTS ENTRY LAUNCHER: one line per refusal on stdout; rc 0 only when there is none.
#   - ENTRY, the PATH entry, reaches the launcher as a link (the launcher walks $0) or as a pnpm
#     shim, which must be exactly the text npm_pnpm_shim gives for the target and NODE_PATH it
#     names (neither may hold a quote, $, backquote or backslash), and whose `$basedir//bin/sh`,
#     which it would run instead, must not exist; each launcher it reaches is byte-equal LAUNCHER
#     and resolves to one of INSTS (real paths, one per line);
#   - every copy of the launcher at a package path under HOME (*/sheepr/bin/sheepr;
#     bun's cache and pnpm's store keep theirs under other names, and only ENTRY is ever run) is
#     byte-equal LAUNCHER, and a copy that resolves to an executable resolves to one of INSTS (one
#     that resolves to nothing runs nothing).
# Any refusal stands, whatever order the copies are found in.
npm_gate() {
  ng_bad=0 ng_nl='
'
  if [ -L "$3" ]; then ng_a0=$3
  elif [ -f "$3" ] && grep -q '^basedir=' "$3"; then
    ng_a0=""
    ng_t=$(sed -n 's/^  exec \/bin\/sh  "\$basedir\/\(.*\)" "\$@"$/\1/p' "$3")
    ng_np=$(sed -n 's/^  export NODE_PATH="\(.*\)"$/\1/p' "$3" | head -1)
    case $ng_t$ng_np in
      *[\"\$\`\\]*|'') echo "the pnpm shim $3 names a target or NODE_PATH with a quote, \$, backquote or backslash, or none" ;;
      *) if npm_pnpm_shim "$ng_t" "$ng_np" | cmp -s - "$3"; then ng_a0=$(dirname "$3")/$ng_t
         else echo "the pnpm shim $3 is not the text pnpm 10.18.2 writes"; fi ;;
    esac
    [ -n "$ng_a0" ] || ng_bad=1
    if [ -e "$(dirname "$3")//bin/sh" ] || [ -L "$(dirname "$3")//bin/sh" ]; then echo "the pnpm shim $3 would run $(dirname "$3")/bin/sh"; ng_bad=1; fi
  else ng_a0=""; echo "the PATH entry $3 is neither a link nor a pnpm shim"; ng_bad=1; fi
  ng_cp=$(find "$1" -path '*/sheepr/bin/sheepr' -type f)
  ng_ifs=$IFS; IFS=$ng_nl; set -f
  for ng_w in $(printf '%s\n' "$ng_a0" | sed 's/^/entry /'; printf '%s\n' "$ng_cp" | sed 's/^/copy /'); do
    ng_k=${ng_w%% *} ng_a=${ng_w#* }
    [ -n "$ng_a" ] || continue
    ng_lf=$(npm_walk "$ng_a")
    if ! cmp -s "$ng_lf" "$4"; then echo "the launcher $ng_lf is not the package's"; ng_bad=1; continue; fi
    if ng_t=$(npm_target "$ng_a"); then
      printf '%s\n' "$2" | grep -qxF "$ng_t" || { echo "the launcher $ng_lf would run $ng_t, which the door did not judge"; ng_bad=1; }
    elif [ "$ng_k" = entry ]; then echo "the launcher $ng_lf that $3 reaches resolves to no executable"; ng_bad=1; fi
  done
  IFS=$ng_ifs; set +f
  [ $ng_bad = 0 ]
}
