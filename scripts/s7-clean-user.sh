#!/bin/sh
# The clean-user leg of S7 (PHASE3.md S7, "A clean Mac user"): needs no repo and no cargo once
# `prep` has run. Its cell: tests/dist/t_s7_clean_user.sh.
#
#   The operator (an admin), from the repo:
#     s7-clean-user.sh prep RC-DIR     make /Users/Shared/sr-s7 from a signed rc's output directory
#     s7-clean-user.sh brew-after      after the leg: nothing of it reached /opt/homebrew or your
#                                      own Sheepr files
#   The test user (a standard user), as `sh /Users/Shared/sr-s7/s7-clean-user.sh ...`:
#     tools                            Homebrew, node and pnpm (pinned, checked) into this home
#     channel npm|pnpm|cask|install-sh install one way, check it, then the Full Disk Access steps
#     uninstall CHANNEL                remove it again (run it again to finish an interrupted one;
#                                      --operator-removed-grant only when its entry is gone)
#     finish                           leave this user ready for the post-publish check (§5 step 6)
#
# The wall: `prep` and `brew-after` refuse a user not in `admin`; every other subcommand refuses a
# user in `admin`, the owner of /Users/Shared/sr-s7, or a missing /Users/Shared/sr-s7, with exit 3,
# before it writes anything. Its inputs come from /usr/bin/id and /usr/bin/stat only.
# SR_S7_DIR and SR_S7_BREW move `prep`'s and `brew-after`'s directory and Homebrew prefix (for
# the cell); the test user's subcommands always use /Users/Shared/sr-s7. SR_S7_LIB=1 when the file
# is sourced defines the functions and runs nothing.
# The state, ~/.s7-channel: `channel=`, `entry=`, a `bundle=` per bundle found, and `granted=yes`
# whenever a grant may be live. The flag lives in that file only: every read sets it first and only
# a denial clears it (so a failed write leaves it set); the grant prompt sets it, and the read just
# after the grant does not clear it (a click after that read may still add one). While it is set,
# uninstall (and so finish and the next channel) refuses until a read through the entry is denied;
# when the entry is gone, only `uninstall CHANNEL --operator-removed-grant`, the operator's
# statement, recorded first, clears it.
set -u
S7_ID=com.lukaso.sheepr
S7_DIR=/Users/Shared/sr-s7
S7_LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
# pinned, measured 2026-10-02 (PHASE3.md S7)
S7_BREW_TAG=7.0.7
S7_BREW_SHA=1e7a927dc83fe9d2e8cc58d761d8ee0499e4c660164e68d6b9f70a705748d930
S7_NODE=v24.9.0
S7_NODE_SHA=961024296c2a8e60daed0784f8b61e0fab5c51d197502a92eff052c72b53209b
S7_PNPM=10.18.2
S7_PNPM_PIN=pnpm@10.18.2+sha512.9fb969fa749b3ade6035e0f109f0b8a60b5d08a1a87fdf72e337da90dcc93336e2280ca4e44f2358a649b83c17959e9993e777c2080879f3801e6f0d999ad3dd
# the outside effects, by absolute path; plain assignments (no environment variable reaches them):
# only a shell that sources this file in library mode can point them at stand-ins (its cell does)
S7_TCCUTIL=/usr/bin/tccutil S7_MDFIND=/usr/bin/mdfind S7_CURL=/usr/bin/curl S7_TTY=/dev/tty S7_TTY_OUT=/dev/tty

s7_die() { echo "s7: $*" >&2; exit 1; }

# s7_wall GROUPS UID OWNER-UID ROLE: 0 when ROLE (operator | user) may run; otherwise prints why
s7_wall() {
  case $4 in
    operator)
      case " $1 " in *" admin "*) return 0 ;; esac
      echo "this is the operator's step: run it as an admin user"; return 1 ;;
    user)
      case " $1 " in *" admin "*) echo "this is the test user's step, and this user is an admin (it could reach the operator's home or Homebrew)"; return 1 ;; esac
      [ -n "$3" ] || { echo "$S7_DIR is missing (the operator runs prep first)"; return 1; }
      [ "$2" != "$3" ] || { echo "the owner of $S7_DIR is the operator, not the test user"; return 1; }
      return 0 ;;
  esac
  echo "no role $4"; return 1
}
# s7_wall_inputs DIR: the wall's inputs, one per line: this user's groups, its uid, and DIR's
# owner uid (empty when DIR is missing or a link); read only from /usr/bin/id and /usr/bin/stat
s7_wall_inputs() {
  /usr/bin/id -Gn; /usr/bin/id -u
  if [ -d "$1" ] && [ ! -L "$1" ]; then /usr/bin/stat -f %u "$1"; else echo; fi
}
# s7_walled ROLE DIR: exit 3 unless the wall allows this user (the entry passes /Users/Shared/sr-s7)
s7_walled() {
  w_i=$(s7_wall_inputs "$2"; echo .)
  w_g=$(printf '%s\n' "$w_i" | sed -n 1p) w_u=$(printf '%s\n' "$w_i" | sed -n 2p) w_o=$(printf '%s\n' "$w_i" | sed -n 3p)
  w_r=$(s7_wall "$w_g" "$w_u" "$w_o" "$1") || { echo "s7: refused: $w_r" >&2; exit 3; }
}


# s7_classify EXIT STATUS-FILE STDERR-FILE DIR: denied | allowed | bad: why (a read of DIR)
s7_classify() {
  c_s=$(cat "$2" 2>/dev/null)
  case $c_s in *'"tracking":"responsibility"'*) ;; *) echo "bad: not responsibility tracking: ${c_s:-no status line}"; return 0 ;; esac
  case $c_s in *'"degraded":null'*) ;; *) echo "bad: degraded: $c_s"; return 0 ;; esac
  case $c_s in *'"error":null'*) ;; *) echo "bad: an error: $c_s"; return 0 ;; esac
  if grep -qF "$4: Operation not permitted" "$3"; then echo denied
  elif [ "$1" = 0 ]; then echo allowed
  else echo "bad: exit $1: $(head -2 "$3" | tr '\n' ' ')"; fi
}
# s7_read_one ENTRY TMPDIR: the protected read through ENTRY, classified (details in TMPDIR/s, /e, /x)
s7_read_one() {
  "$1" run --timeout 20s --status-fd 3 -- ls "$HOME/Library/Safari" 3> "$2/s" > "$2/o" 2> "$2/e"; echo $? > "$2/x"
  s7_classify "$(cat "$2/x")" "$2/s" "$2/e" "$HOME/Library/Safari"
}

# s7_ls_paths DUMP: the paths of the Launch Services records whose identifier is exactly S7_ID
s7_ls_paths() {
  awk -v id="$S7_ID" '
    function out() { if (ok && p != "") print p; ok = 0; p = "" }
    /^-----------/ { out(); next }
    /^path:/ { v = $0; sub(/^path:[ \t]+/, "", v); sub(/ \(0x[0-9a-f]+\)$/, "", v); p = v; next }
    /^identifier:/ { v = $0; sub(/^identifier:[ \t]+/, "", v); if (v == id) ok = 1; next }
    END { out() }' "$1" | LC_ALL=C sort -u
}
# s7_ls_ok DUMP BUNDLE: no record, or exactly one at BUNDLE (prints the records)
s7_ls_ok() {
  l_p=$(s7_ls_paths "$1"); printf '%s\n' "${l_p:-(no record)}"
  [ -z "$l_p" ] || [ "$l_p" = "$2" ]
}

# s7_cask_local IN URL OUT: the cask with only its one url line replaced
s7_cask_local() {
  [ "$(grep -c '^  url "' "$1")" = 1 ] || { echo "s7: $1 must have exactly one url line" >&2; return 1; }
  case $2 in *[\|\&\\\"]*|'') echo "s7: unusable url $2" >&2; return 1 ;; esac
  sed "s|^  url \".*\"\$|  url \"$2\"|" "$1" > "$3"
}

# the channel state (see the header)
s7_state_free() { [ ! -e "$HOME/.s7-channel" ]; }
s7_state_get() { sed -n "s/^$1=//p" "$HOME/.s7-channel" 2>/dev/null; }
s7_state_is() { [ "$(s7_state_get channel)" = "$1" ]; }
s7_finish_ok() { s7_state_free; }
s7_state_put() { # channel entry bundles(one per line): the grant flag already in the file is kept
  p_g=$(s7_state_get granted)
  { echo "channel=$1"; echo "entry=$2"; [ "$p_g" != yes ] || echo "granted=yes"; printf '%s\n' "$3" | sed '/^$/d; s/^/bundle=/'; } > "$HOME/.s7-channel.new" \
    && mv "$HOME/.s7-channel.new" "$HOME/.s7-channel"
}
s7_flag() { # yes | no: the grant flag, in the state file only (the one place it lives)
  { grep -v '^granted=' "$HOME/.s7-channel"; [ "$1" != yes ] || echo "granted=yes"; } > "$HOME/.s7-channel.new" \
    && mv "$HOME/.s7-channel.new" "$HOME/.s7-channel"
}

# the test user's tools, by their own entry files (never looked up through PATH)
s7_brew() { "$HOME/homebrew/bin/brew" "$@"; }
# s7_deprecations FILE: 0 when brew's output there names no deprecation (the shipped cask must load
# without one: --strict audits do not flag them, and rc.2's cask printed one only at uninstall)
s7_deprecations() { ! grep -qi 'deprecat' "$1"; }
s7_node() { "$HOME/node/bin/node" "$@"; }
s7_npm() { "$HOME/node/bin/node" "$HOME/node/lib/node_modules/npm/bin/npm-cli.js" "$@"; }
s7_corepack() { "$HOME/node/bin/node" "$HOME/node/lib/node_modules/corepack/dist/corepack.js" "$@"; }
s7_pnpm() { "$HOME/node/bin/node" "$HOME/corepack/v1/pnpm/$S7_PNPM/bin/pnpm.cjs" "$@"; }

# s7_uninstall CHANNEL [--operator-removed-grant]: in the leg's environment; a grant that may
# remain blocks it until a read through the entry is denied (with the entry gone: the operator's
# recorded statement); then lsregister -u of every bundle that exists, the README's
# removal, and no record left at a bundle's path (a record left is unregistered again, the bundle
# gone or not), or the state stays; every step skips what is already gone, so a rerun finishes
s7_uninstall() {
  s7_state_is "$1" || { echo "s7: the installed channel is '$(s7_state_get channel)', not '$1'" >&2; return 1; }
  [ ! -f "$HOME/s7-env.sh" ] || . "$HOME/s7-env.sh"
  u_bl=$(s7_state_get bundle) u_e=$(s7_state_get entry) u_dep=""
  if [ "$(s7_state_get granted)" = yes ]; then
    if [ -n "$u_e" ] && [ -x "$u_e" ]; then
      u_t=$(mktemp -d "${TMPDIR:-/tmp}/s7-un.XXXXXX") || return 1
      u_k=$(s7_read_one "$u_e" "$u_t"); rm -rf "$u_t"
      if [ "$u_k" != denied ]; then
        echo "s7: a Full Disk Access grant for Sheepr may remain (the read: $u_k). Remove the Sheepr row with − in Full Disk Access; if no row shows, the operator runs, in their own account: tccutil reset SystemPolicyAllFiles $S7_ID. Then run 'uninstall $1' again." >&2
        return 1
      fi
      s7_flag no || return 1
      [ ! -w "$S7_DIR/results" ] || echo "  uninstall: the read is denied: no grant remains" >> "$S7_DIR/results/$1.txt"
    elif [ "${2:-}" = --operator-removed-grant ]; then
      { [ -w "$S7_DIR/results" ] && echo "  uninstall: the entry is gone, so no read is possible: the operator states the grant was removed" >> "$S7_DIR/results/$1.txt"; } \
        || { echo "s7: the statement cannot be recorded in $S7_DIR/results: the grant flag stays" >&2; return 1; }
      s7_flag no || return 1
    else
      echo "s7: a Full Disk Access grant for Sheepr may remain, and the entry ($u_e) is gone, so no read can show it is not. When the operator has removed it (tccutil reset SystemPolicyAllFiles $S7_ID in their own account, or − in their own System Settings), run: uninstall $1 --operator-removed-grant" >&2
      return 1
    fi
  fi
  printf '%s\n' "$u_bl" | while IFS= read -r u_b; do [ -n "$u_b" ] && [ -d "$u_b" ] && "$S7_LSREG" -u "$u_b"; done
  case $1 in
    install-sh) rm -rf "${HOME:?}/Applications/Sheepr.app" "${HOME:?}/.local/bin/sheepr" ;;
    cask) if [ -d "$HOME/homebrew/Caskroom/sheepr" ]; then
            u_o=$(mktemp "${TMPDIR:-/tmp}/s7-brew.XXXXXX") || return 1
            s7_brew uninstall --cask s7/local/sheepr > "$u_o" 2>&1 || { cat "$u_o" >&2; rm -f "$u_o"; return 1; }
            cat "$u_o"; s7_deprecations "$u_o" || u_dep=$(tr '\n' ' ' < "$u_o"); rm -f "$u_o"
          fi ;;
    npm) if [ -d "$HOME/npm-global/lib/node_modules/sheepr" ]; then s7_npm rm -g sheepr || return 1; fi ;;
    pnpm)
      for u_x in "${HOME:?}"/pnpm/global/*/node_modules/sheepr; do
        [ -e "$u_x" ] || [ -L "$u_x" ] || continue
        s7_pnpm rm -g sheepr || return 1; break
      done
      # pnpm rm -g leaves both packages in its global virtual store (measured, pnpm 10.18.2), the
      # bundle included: recorded (the README's line leaves it too), then removed
      for u_x in "${HOME:?}"/pnpm/global/*/.pnpm/sheepr@* "${HOME:?}"/pnpm/global/*/.pnpm/sheepr-darwin-universal@*; do
        [ -e "$u_x" ] || continue
        echo "s7: pnpm rm -g left $u_x: removing it"
        [ ! -w "$S7_DIR/results" ] || echo "  uninstall: pnpm rm -g left $u_x (removed by the helper)" >> "$S7_DIR/results/pnpm.txt"
        rm -rf "$u_x"
      done ;;
    *) echo "s7: no channel $1" >&2; return 1 ;;
  esac
  u_d=$(mktemp "${TMPDIR:-/tmp}/s7-dump.XXXXXX") || return 1
  u_left=$(printf '%s\n' "$u_bl" | while IFS= read -r u_b; do
    [ -n "$u_b" ] || continue
    if [ -e "$u_b" ]; then echo "$u_b is still there"; continue; fi
    "$S7_LSREG" -dump > "$u_d" 2>/dev/null
    if s7_ls_paths "$u_d" | grep -qxF "$u_b"; then
      "$S7_LSREG" -u "$u_b"; "$S7_LSREG" -dump > "$u_d" 2>/dev/null
      ! s7_ls_paths "$u_d" | grep -qxF "$u_b" || echo "Launch Services still has a record at $u_b"
    fi
  done)
  rm -f "$u_d"
  [ -z "$u_left" ] || { printf 's7: %s\n' "$u_left" >&2; echo "s7: run 'uninstall $1' again" >&2; return 1; }
  rm -f "$HOME/.s7-channel"
  echo "s7: $1 removed"
  # removed, and the state cleared, but the cask's text is not clean: say so and fail
  if [ -n "$u_dep" ]; then
    echo "s7: brew uninstall printed a deprecation; the shipped cask must not: $u_dep" >&2
    [ ! -w "$S7_DIR/results" ] || echo "  uninstall: brew printed a deprecation: $u_dep" >> "$S7_DIR/results/cask.txt"
    return 1
  fi
}

# s7_listing PREFIX HOME: what brew-after compares (sorted); fails if a part cannot be read
s7_listing() {
  l_t=$(mktemp "${TMPDIR:-/tmp}/s7-list.XXXXXX") || return 1
  {
    echo "prefix $1"
    for l_s in Caskroom bin; do
      [ -d "$1/$l_s" ] || { echo "absent $l_s"; continue; }
      for l_e in "$1/$l_s"/* "$1/$l_s"/.[!.]*; do
        [ -e "$l_e" ] || [ -L "$l_e" ] || continue
        if [ -L "$l_e" ]; then echo "entry $l_s/${l_e##*/} -> $(readlink "$l_e")"; else echo "entry $l_s/${l_e##*/}"; fi
      done
    done
    for l_u in "$1/Library/Taps"/*; do
      [ -d "$l_u" ] || continue
      echo "entry Library/Taps/${l_u##*/}"
      for l_r in "$l_u"/*; do [ -e "$l_r" ] && echo "entry Library/Taps/${l_u##*/}/${l_r##*/}"; done
    done
    if [ -L "$2/.local/bin/sheepr" ]; then echo "ophome-link $(readlink "$2/.local/bin/sheepr")"
    elif [ -e "$2/.local/bin/sheepr" ]; then echo "ophome-link a file"; else echo "ophome-link absent"; fi
  } > "$l_t" || { rm -f "$l_t"; return 1; }
  python3 - "$2/Applications/Sheepr.app" >> "$l_t" <<'PY' || { rm -f "$l_t"; return 1; }
import os, sys, stat, hashlib
r = sys.argv[1]
if not os.path.lexists(r): print("ophome-app absent"); sys.exit()
h = hashlib.sha256()
def one(p, k):
    st = os.lstat(p); v = "%s %o %o" % (k, stat.S_IFMT(st.st_mode), st.st_mode & 0o7777)
    if stat.S_ISREG(st.st_mode): v += " " + hashlib.sha256(open(p, "rb").read()).hexdigest()
    elif stat.S_ISLNK(st.st_mode): v += " " + os.readlink(p)
    h.update(v.encode() + b"\n")
one(r, ".")
if os.path.isdir(r) and not os.path.islink(r):
    for d, ds, fs in os.walk(r):
        ds.sort()
        for n in sorted(ds + fs): p = os.path.join(d, n); one(p, os.path.relpath(p, r))
print("ophome-app " + h.hexdigest())
PY
  LC_ALL=C sort "$l_t"; l_r=$?; rm -f "$l_t"; return $l_r
}

s7_cmd_prep() {
  s7_walled operator "$S7_DIR"
  [ $# = 1 ] || { echo "usage: s7-clean-user.sh prep RC-DIR" >&2; exit 2; }
  p_rc=$(cd "$1" 2>/dev/null && pwd -P) || s7_die "no directory $1"
  p_d=${SR_S7_DIR:-/Users/Shared/sr-s7} p_b=${SR_S7_BREW:-/opt/homebrew}
  p_root=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd -P)
  for p_f in scripts/lib/exec-guard.sh scripts/release.conf scripts/lib/static-registry.py scripts/lib/render-cask.sh scripts/smoke.sh tests/lib/tree-same.py; do
    [ -f "$p_root/$p_f" ] || s7_die "run prep from the repo's scripts/s7-clean-user.sh (no $p_f)"
  done
  if [ -e "$p_d" ] || [ -L "$p_d" ]; then s7_die "$p_d exists: remove it first"; fi
  # the rc: a signed release's directory, named for its tag, every file as its lists say
  p_m=$(python3 - "$p_rc" <<'PY'
import sys, os, json, hashlib
d = sys.argv[1]; m = json.load(open(os.path.join(d, "MANIFEST.json")))
tag = os.path.basename(d)
if m.get("mode") != "signed" or m.get("control") is not False: sys.exit("the manifest is not a signed release's (mode %r, control %r)" % (m.get("mode"), m.get("control")))
if m.get("tag") != tag: sys.exit("the manifest's tag %r is not the directory's name %r" % (m.get("tag"), tag))
v = m["tag"][1:]; files = {f["name"]: f["sha256"] for f in m["files"]}
need = ["sheepr-macos-universal.tar.gz", "install.sh"] + ["%s-%s.tgz" % (p, v) for p in ("sheepr", "sheepr-darwin-universal", "sheepr-linux-arm64", "sheepr-linux-x64")]
for n in need:
    if n not in files: sys.exit("the manifest does not list %s" % n)
    if hashlib.sha256(open(os.path.join(d, n), "rb").read()).hexdigest() != files[n]: sys.exit("%s is not the manifest's" % n)
print(v, files["sheepr-macos-universal.tar.gz"])
PY
) || s7_die "$p_rc: refused"
  p_v=${p_m% *} p_sha=${p_m#* }
  (cd "$p_rc" && /usr/bin/shasum -a 256 --strict -c SHA256SUMS > /dev/null 2>&1) || s7_die "$p_rc: SHA256SUMS does not hold"
  p_t=$(mktemp -d "${TMPDIR:-/tmp}/s7-prep.XXXXXX") || exit 1
  /usr/bin/tar -xzOf "$p_rc/sheepr-macos-universal.tar.gz" Sheepr.app/Contents/Info.plist > "$p_t/Info.plist" 2>/dev/null || { rm -rf "$p_t"; s7_die "$p_rc: no Info.plist in the archive"; }
  if /usr/bin/plutil -extract SheeprControlBuild raw "$p_t/Info.plist" > /dev/null 2>&1; then rm -rf "$p_t"; s7_die "$p_rc: the archive's bundle carries the control marker"; fi
  sh "$p_root/scripts/lib/render-cask.sh" "$p_v" "$p_sha" "$p_t/sheepr.rb" && cmp -s "$p_t/sheepr.rb" "$p_rc/sheepr.rb" \
    || { rm -rf "$p_t"; s7_die "$p_rc: sheepr.rb is not the cask rendered for $p_v"; }
  rm -rf "$p_t"
  mkdir "$p_d" || s7_die "cannot make $p_d"
  mkdir "$p_d/lib" "$p_d/door" "$p_d/door/lib" "$p_d/npm" "$p_d/results" || s7_die "cannot fill $p_d"
  cp "$p_rc/install.sh" "$p_rc/SHA256SUMS" "$p_rc/MANIFEST.json" "$p_rc/sheepr-macos-universal.tar.gz" "$p_rc/sheepr.rb" "$p_d/" \
    && cp "$p_rc/sheepr-$p_v.tgz" "$p_rc/sheepr-darwin-universal-$p_v.tgz" "$p_rc/sheepr-linux-arm64-$p_v.tgz" "$p_rc/sheepr-linux-x64-$p_v.tgz" "$p_d/npm/" \
    && cp "$p_root/scripts/smoke.sh" "$p_d/smoke.sh" && cp "$0" "$p_d/s7-clean-user.sh" \
    && cp "$p_root/scripts/lib/static-registry.py" "$p_root/tests/lib/tree-same.py" "$p_d/lib/" \
    && cp "$p_root/scripts/lib/exec-guard.sh" "$p_d/door/lib/exec-guard.sh" && cp "$p_root/scripts/release.conf" "$p_d/door/release.conf" \
    || s7_die "copying into $p_d failed"
  chmod -R a+rX,go-w "$p_d" && chmod 1777 "$p_d/results" || s7_die "cannot set the modes in $p_d"
  s7_listing "$p_b" "$HOME" > "$p_d/brew-before.txt" || s7_die "cannot list $p_b"
  chmod a+r "$p_d/brew-before.txt"
  echo "s7: $p_d is ready for the test user ($p_v)."
}

s7_cmd_brew_after() {
  s7_walled operator "$S7_DIR"
  a_d=${SR_S7_DIR:-/Users/Shared/sr-s7} a_b=${SR_S7_BREW:-/opt/homebrew}
  [ -f "$a_d/brew-before.txt" ] || s7_die "no $a_d/brew-before.txt"
  a_p=$(sed -n 's/^prefix //p' "$a_d/brew-before.txt")
  echo "s7: reading $a_b"
  [ "$a_p" = "$a_b" ] || { echo "s7: brew-before.txt was taken from $a_p, not $a_b" >&2; exit 2; }
  a_now=$(s7_listing "$a_b" "$HOME") || s7_die "cannot list $a_b or your own Sheepr files"
  a_diff=$(printf '%s\n' "$a_now" | diff "$a_d/brew-before.txt" - | grep '^[<>]')
  [ -n "$a_diff" ] || { echo "s7: nothing changed in $a_b or in your own Sheepr files"; return 0; }
  a_pat='sheepr|Library/Taps/s7(/|$)|^[<>] ophome-|^[<>] prefix '
  printf '%s\n' "$a_diff" | grep -viE "$a_pat" | sed 's/^/s7: information, not the leg'"'"'s: /'
  a_hard=$(printf '%s\n' "$a_diff" | grep -iE "$a_pat")
  [ -z "$a_hard" ] || { printf '%s\n' "$a_hard" | sed 's/^/s7: CHANGED: /' >&2; exit 1; }
  echo "s7: nothing of the leg's in $a_b or in your own Sheepr files"
}

s7_fetch() { # url sha256 out
  "$S7_CURL" -fsSL -o "$3" "$1" || s7_die "cannot download $1"
  [ "$(/usr/bin/shasum -a 256 "$3" | cut -d' ' -f1)" = "$2" ] || s7_die "$1: its sha256 is not the pinned $2"
}
s7_env() {
  cat <<'EOF'
# written by s7-clean-user.sh tools: the clean-user leg's environment (PHASE3.md S7)
export PATH="$HOME/npm-global/bin:$HOME/pnpm:$HOME/homebrew/bin:$HOME/.local/bin:$HOME/node/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 HOMEBREW_NO_ENV_HINTS=1
export HOMEBREW_TEMP="${TMPDIR:-/tmp}"
export NPM_CONFIG_PREFIX="$HOME/npm-global" PNPM_HOME="$HOME/pnpm"
export COREPACK_HOME="$HOME/corepack" COREPACK_ENABLE_NETWORK=0
EOF
}

# tools: ~/s7-env.sh is written last (it is what says tools finished); a run that did not finish
# leaves ~/s7-env.sh.part, and the next run names what to remove
s7_cmd_tools() {
  s7_walled user "$S7_DIR"
  [ ! -e "$HOME/s7-env.sh" ] || s7_die "tools has run (~/s7-env.sh exists)"
  for t_x in homebrew node corepack s7-env.sh.part; do
    [ ! -e "$HOME/$t_x" ] || s7_die "an earlier tools run did not finish: remove ~/homebrew ~/node ~/corepack ~/s7-env.sh.part (those that exist), then run tools again"
  done
  [ "$(/usr/bin/uname -m)" = arm64 ] || s7_die "the leg is pinned to arm64 (node darwin-arm64)"
  [ -d "$HOME/Library/Safari" ] || s7_die "open Safari once and quit it first, so ~/Library/Safari (the reads' folder) exists"
  t_t=$(mktemp -d "${TMPDIR:-/tmp}/s7-tools.XXXXXX") || exit 1
  trap 'rm -rf "$t_t"' EXIT
  s7_fetch "https://github.com/Homebrew/brew/archive/refs/tags/$S7_BREW_TAG.tar.gz" "$S7_BREW_SHA" "$t_t/brew.tgz"
  s7_fetch "https://nodejs.org/dist/$S7_NODE/node-$S7_NODE-darwin-arm64.tar.gz" "$S7_NODE_SHA" "$t_t/node.tgz"
  s7_env > "$HOME/s7-env.sh.part" && . "$HOME/s7-env.sh.part" || s7_die "cannot write ~/s7-env.sh.part"
  mkdir "$HOME/homebrew" "$HOME/node" && /usr/bin/tar -xzf "$t_t/brew.tgz" --strip-components 1 -C "$HOME/homebrew" \
    && /usr/bin/tar -xzf "$t_t/node.tgz" --strip-components 1 -C "$HOME/node" || s7_die "unpacking Homebrew or node failed"
  COREPACK_ENABLE_NETWORK=1 s7_corepack install -g "$S7_PNPM_PIN" || s7_die "corepack could not fetch $S7_PNPM_PIN"
  [ "$(s7_pnpm -v)" = "$S7_PNPM" ] || s7_die "pnpm is not $S7_PNPM"
  s7_brew vendor-install ruby && s7_brew config > /dev/null && s7_brew install-bundler-gems --groups=audit,ast,style \
    || s7_die "Homebrew's first-run fetches failed"
  echo "s7: $(s7_brew --version | head -1), node $(s7_node -v), npm $(s7_npm -v), pnpm $(s7_pnpm -v)"
  mv "$HOME/s7-env.sh.part" "$HOME/s7-env.sh" || s7_die "cannot write ~/s7-env.sh"
  echo "s7: next: sh $S7_DIR/s7-clean-user.sh channel npm"
}

# the rest of a channel run (they set and read c_* variables)
s7_rec() { printf '%s\n' "$*" | tee -a "$c_r"; }
s7_save() { s7_state_put "$c_c" "${c_e:-}" "${c_bl:-}"; }
s7_stop() { s7_rec "STOPPED at $1"; echo "s7: the channel stopped; run 'uninstall $c_c' before another channel" >&2; exit 1; }
s7_ask() { # question -> y or n, recorded; no answer (end of input) returns 1 with nothing printed
  while :; do
    printf '%s [y/n] ' "$1" > "$S7_TTY_OUT"
    read -r c_a <&9 || { printf '  asked: %s -> (no answer)\n' "$1" >> "$c_r"; return 1; }
    case $c_a in y|n) break ;; esac
  done
  printf '  asked: %s -> %s\n' "$1" "$c_a" >> "$c_r"; printf '%s\n' "$c_a"
}
s7_say() { printf '\n>>> %s\n' "$*" > "$S7_TTY_OUT"; }
s7_read() { # [keep] -> denied | allowed | bad: ..., recorded. The grant flag is set before the
  # read and cleared only by a denial (not with keep: the operator has just been asked to grant, and
  # a click after the read may still add one), so a failed write leaves it set
  s7_flag yes || { echo "bad: cannot record the grant flag (the state file cannot be written): a grant may remain"; return 0; }
  r_k=$(s7_read_one "$c_e" "$c_t")
  printf '  read: %s (exit %s); status %s; stderr %s\n' "$r_k" "$(cat "$c_t/x")" "$(cat "$c_t/s" 2>/dev/null)" "$(head -3 "$c_t/e" | tr '\n' ' ')" >> "$c_r"
  if [ "$r_k" = denied ] && [ "${1:-}" != keep ]; then s7_flag no || r_k="bad: cannot record the grant flag (the state file cannot be written): a grant may remain"; fi
  printf '%s\n' "$r_k"
}
s7_why() { # read-result reason: why the channel stops at a read (a failed state write names itself)
  case $1 in "bad: cannot record"*) printf '%s\n' "$1" ;; *) printf '%s: %s\n' "$2" "$1" ;; esac
}
s7_need() { # want step reason [keep]: a read that must give WANT, else the channel stops at STEP
  n_k=$(s7_read_clear "${4:-}")
  [ "$n_k" = "$1" ] || s7_stop "$2: $(s7_why "$n_k" "$3")"
}
s7_done() { # step: asks "Done?" until the answer is y (a "n" is "not yet": no read, no try)
  d_a=n; while [ "$d_a" != y ]; do d_a=$(s7_ask "$1 Done?") || s7_stop "$1: no answer"; done
}
s7_read_clear() { # [keep]: a read, again while it is unclear (at most 3 tries)
  i=0; while :; do r_c=$(s7_read "${1:-}"); case $r_c in denied|allowed|"bad: cannot record"*) break ;; esac; i=$((i + 1)); [ $i -lt 3 ] || break; done
  printf '%s\n' "$r_c"
}
s7_dump() { "$S7_LSREG" -dump > "$c_t/dump" 2>/dev/null; s7_ls_ok "$c_t/dump" "$c_b"; }
s7_serve() { # dir -> starts a loopback server on a free port; c_port, c_srv
  c_port=$(/usr/bin/python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
  (cd "$1" && exec /usr/bin/python3 -m http.server "$c_port" --bind 127.0.0.1) > "$c_t/srv.log" 2>&1 & c_srv=$!
  i=0; until /usr/bin/curl -fs "http://127.0.0.1:$c_port/" > /dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.2; i=$((i + 1)); done
  [ $i -le 50 ] || s7_stop "install: the loopback server did not start"
}

s7_cmd_channel() {
  s7_walled user "$S7_DIR"
  c_c=${1:-}
  case $c_c in npm|pnpm|cask|install-sh) ;; *) echo "usage: s7-clean-user.sh channel npm|pnpm|cask|install-sh" >&2; exit 2 ;; esac
  s7_state_free || s7_die "channel '$(s7_state_get channel)' is installed: uninstall it first"
  [ -f "$HOME/s7-env.sh" ] || s7_die "run tools first (~/s7-env.sh is written when it finishes)"
  . "$HOME/s7-env.sh"
  [ -d "$HOME/Library/Safari" ] || s7_die "~/Library/Safari is missing: open Safari once and quit it"
  exec 9< "$S7_TTY" || s7_die "cannot read answers from $S7_TTY"
  c_e="" c_bl="" c_b=""
  s7_save   # before the channel's first write: uninstall can always clean up
  c_r=$S7_DIR/results/$c_c.txt
  c_t=$(mktemp -d "${TMPDIR:-/tmp}/s7-channel.XXXXXX") || exit 1
  c_srv=""; trap '[ -z "$c_srv" ] || kill "$c_srv" 2>/dev/null; rm -rf "$c_t"' EXIT
  c_v=$(python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); print(m["tag"][1:], m["commit"][:12])' "$S7_DIR/MANIFEST.json") || s7_die "no manifest"
  c_ver=${c_v% *} c_commit=${c_v#* }
  s7_rec "== channel $c_c, $(date -u +%Y-%m-%dT%H:%M:%SZ), user $(id -un), rc $c_ver ($c_commit)"
  case $c_c in
    npm|pnpm)
      rm -rf "$HOME/s7-reg"; mkdir -p "$HOME/s7-reg"
      s7_serve "$HOME/s7-reg"
      /usr/bin/python3 "$S7_DIR/lib/static-registry.py" "$HOME/s7-reg" "$c_port" "" "$S7_DIR/npm" || s7_stop "install: the registry"
      printf 'registry=http://127.0.0.1:%s/\n' "$c_port" > "$HOME/s7-reg/npmrc"
      if [ "$c_c" = npm ]; then
        c_e=$HOME/npm-global/bin/sheepr c_root=$HOME/npm-global; s7_save
        (export npm_config_userconfig="$HOME/s7-reg/npmrc"; s7_npm i -g --registry "http://127.0.0.1:$c_port/" "sheepr@$c_ver") > "$c_t/inst" 2>&1 \
          || { cat "$c_t/inst" >&2; s7_stop "install: npm i -g failed"; }
      else
        c_e=$HOME/pnpm/sheepr c_root=$HOME/pnpm; s7_save
        (export npm_config_userconfig="$HOME/s7-reg/npmrc"; s7_pnpm add -g --registry "http://127.0.0.1:$c_port/" "sheepr@$c_ver") > "$c_t/inst" 2>&1 \
          || { cat "$c_t/inst" >&2; s7_stop "install: pnpm add -g failed"; }
      fi
      kill "$c_srv" 2>/dev/null; c_srv=""
      c_bl=$(find "$c_root" -name Sheepr.app -type d -prune | while IFS= read -r c_x; do (cd -P "$c_x" && pwd -P); done); s7_save
      [ "$(printf '%s\n' "$c_bl" | grep -c .)" = 1 ] || s7_stop "install: not exactly one Sheepr.app under $c_root: $(printf '%s ' $c_bl)"
      c_b=$c_bl
      /usr/bin/tar -xzOf "$S7_DIR/npm/sheepr-$c_ver.tgz" package/bin/sheepr > "$c_t/launcher" || s7_stop "install: no launcher in the package"
      find "$c_root" -path '*/sheepr/bin/sheepr' -type f > "$c_t/launchers"
      [ -s "$c_t/launchers" ] || s7_stop "install: no launcher under $c_root"
      while IFS= read -r c_l; do cmp -s "$c_l" "$c_t/launcher" || s7_stop "install: the launcher $c_l is not the package's"; done < "$c_t/launchers" ;;
    cask)
      c_e=$HOME/homebrew/bin/sheepr c_b=$HOME/Applications/Sheepr.app; c_bl=$c_b; s7_save
      c_tap=$HOME/homebrew/Library/Taps/s7/homebrew-local
      [ -d "$c_tap" ] || s7_brew tap-new --no-git s7/local > "$c_t/tap" 2>&1 || { cat "$c_t/tap" >&2; s7_stop "install: brew tap-new"; }
      mkdir -p "$c_tap/Casks" && cp "$S7_DIR/sheepr.rb" "$c_tap/Casks/sheepr.rb" || s7_stop "install: the tap's Casks/"
      s7_brew audit --cask --strict s7/local/sheepr > "$c_t/audit" 2>&1; c_ar=$?
      s7_rec "  brew audit --cask --strict (the shipped text): exit $c_ar: $(tr '\n' ' ' < "$c_t/audit")"
      s7_deprecations "$c_t/audit" || s7_stop "audit: brew printed a deprecation; the shipped cask must load without one"
      s7_cask_local "$S7_DIR/sheepr.rb" "file://$S7_DIR/sheepr-macos-universal.tar.gz" "$c_tap/Casks/sheepr.rb" || s7_stop "install: the cask's local copy"
      s7_brew install --cask --appdir="$HOME/Applications" s7/local/sheepr > "$c_t/inst" 2>&1 || { cat "$c_t/inst" >&2; s7_stop "install: brew install --cask"; }
      s7_deprecations "$c_t/inst" || { cat "$c_t/inst" >&2; s7_stop "install: brew printed a deprecation (above); the shipped cask must load without one"; } ;;
    install-sh)
      c_e=$HOME/.local/bin/sheepr c_b=$HOME/Applications/Sheepr.app; c_bl=$c_b; s7_save
      rm -rf "$HOME/s7-serve"; mkdir -p "$HOME/s7-serve"
      cp "$S7_DIR/SHA256SUMS" "$S7_DIR/sheepr-macos-universal.tar.gz" "$HOME/s7-serve/" || s7_stop "install: the served copy"
      s7_serve "$HOME/s7-serve"
      SHEEPR_INSTALL_BASE="http://127.0.0.1:$c_port" sh "$S7_DIR/install.sh" > "$c_t/inst" 2>&1 || { cat "$c_t/inst" >&2; s7_stop "install: install.sh"; }
      kill "$c_srv" 2>/dev/null; c_srv=""; rm -rf "$HOME/s7-serve" ;;
  esac
  s7_rec "  installed: entry $c_e, bundle $c_b"
  # judged before its first run: the door, then the bundle against the rc archive's
  /bin/sh -p "$S7_DIR/door/lib/exec-guard.sh" check "$c_b/Contents/MacOS/sheepr" 2> "$c_t/door" || s7_stop "the door refuses $c_b: $(cat "$c_t/door")"
  mkdir "$c_t/ref.noindex" && /usr/bin/tar -xzf "$S7_DIR/sheepr-macos-universal.tar.gz" -C "$c_t/ref.noindex" \
    && /usr/bin/python3 "$S7_DIR/lib/tree-same.py" "$c_b" "$c_t/ref.noindex/Sheepr.app" > "$c_t/ts" 2>&1 || s7_stop "the bundle is not the rc archive's: $(cat "$c_t/ts")"
  rm -rf "$c_t/ref.noindex"
  [ "$(command -v sheepr)" = "$c_e" ] || s7_stop "the PATH's sheepr is $(command -v sheepr), not $c_e"
  c_vo=$("$c_e" --version 2>&1)
  case $c_vo in "sheepr ${c_ver%%-*} ($c_commit, macos, responsibility API: active)"*) s7_rec "  --version: $c_vo" ;; *) s7_stop "--version: $c_vo" ;; esac
  sh "$S7_DIR/smoke.sh" "$c_e" < /dev/null > "$c_t/smoke" 2>&1 || { cat "$c_t/smoke" >&2; s7_stop "smoke.sh"; }
  s7_rec "  smoke.sh: passed"
  # the Full Disk Access steps
  s7_rec "  (a) Launch Services records of $S7_ID: $(s7_dump | tr '\n' ' ')"
  s7_dump > /dev/null || s7_stop "(a): a record of $S7_ID that is not this channel's bundle"
  s7_say "Open System Settings > Privacy & Security > Full Disk Access."
  c_row=$(s7_ask "(b) Is there a Sheepr row in the list?") || s7_stop "(b): no answer"
  [ "$c_row" = n ] || { s7_ask "(b) Is it switched on?" > /dev/null || s7_stop "(b): no answer"; }
  s7_need denied "(c)" "the read with no grant is not denied (an earlier grant may answer)"
  c_row=$(s7_ask "(c) After that denied read: is there a Sheepr row now?") || s7_stop "(c): no answer"
  [ "$c_row" = n ] || { s7_ask "(c) Is it switched on?" > /dev/null || s7_stop "(c): no answer"; }
  s7_flag yes || s7_stop "(d): cannot record the grant flag"   # from here until a read is denied, a grant may remain
  s7_say "Grant it: click +, press Cmd-Shift-G, paste $c_b, choose Open (or switch on the Sheepr row that is there). Enter your admin name and password when asked."
  s7_ask "(d) Did you add it with + (y), not by switching on a row that was there (n)?" > /dev/null || s7_stop "(d): no answer"
  s7_ask "(d) Is the Sheepr row shown now?" > /dev/null || s7_stop "(d): no answer"
  s7_need allowed "(e)" "the read with the grant is not allowed" keep
  s7_rec "  (f) Launch Services records before the reset: $(s7_dump | tr '\n' ' ')"
  s7_rec "  (f) mdfind (Spotlight, information only): $("$S7_MDFIND" "kMDItemCFBundleIdentifier == '$S7_ID'" 2>&1 | tr '\n' ' ')"
  "$S7_TCCUTIL" reset SystemPolicyAllFiles "$S7_ID" > "$c_t/tcc" 2>&1; c_tr=$?
  s7_rec "  (f) tccutil reset SystemPolicyAllFiles $S7_ID: exit $c_tr: $(tr '\n' ' ' < "$c_t/tcc")"
  c_k=$(s7_read_clear)
  case $c_k in
    denied) s7_rec "  (g) the reset took: the read is denied" ;;
    allowed)
      s7_rec "  (g) the reset did not take: the read is still allowed"
      i=0
      while :; do
        s7_say "Remove the Sheepr row with − in Full Disk Access. If no row shows here, ask the operator to remove it in their own account: tccutil reset SystemPolicyAllFiles $S7_ID there, or else − in their own System Settings."
        s7_done "(g)"
        c_k=$(s7_read_clear)
        case $c_k in
          denied)
            c_how=$(s7_ask "(g) Was it removed with − here (y), or by the operator in their own account (n)?") || s7_stop "(g): no answer"
            if [ "$c_how" = y ]; then s7_rec "  (g) denied after the removal, with − here"; else s7_rec "  (g) denied after the removal, by the operator in their own account"; fi
            break ;;
          allowed) i=$((i + 1)); [ $i -lt 3 ] || s7_stop "(g): the grant could not be removed; the leg must not go on" ;;
          *) s7_stop "(g): $(s7_why "$c_k" "the read after the removal is not a clear answer, and a grant may remain")" ;;
        esac
      done ;;
    *) s7_stop "(g): $(s7_why "$c_k" "the read after the reset is not a clear answer, so nothing is recorded about the reset, and a grant may remain")" ;;
  esac
  c_row=$(s7_ask "(h) Is the Sheepr row still shown?") || s7_stop "(h): no answer"
  if [ "$c_row" = y ]; then
    s7_say "Remove it with − ."
    s7_done "(h)"
    s7_need denied "(h)" "the read is not denied after the row's removal"
  fi
  s7_rec "  channel $c_c: done"
  echo "s7: next: sh $S7_DIR/s7-clean-user.sh uninstall $c_c"
}

s7_cmd_finish() {
  s7_walled user "$S7_DIR"
  s7_finish_ok || s7_die "channel '$(s7_state_get channel)' is installed: uninstall it first"
  [ -f "$HOME/s7-env.sh" ] || s7_die "run tools first"
  . "$HOME/s7-env.sh"
  if [ -d "$HOME/homebrew/Library/Taps/s7" ]; then s7_brew untap s7/local || s7_die "brew untap s7/local failed"; fi
  s7_brew developer off > /dev/null 2>&1   # audit turns it on (an untar install cannot keep it; off either way)
  if grep -q 's7/local' "$HOME/.homebrew/trust.json" 2>/dev/null; then
    s7_brew untrust --cask s7/local/sheepr > /dev/null 2>&1; s7_brew untrust --tap s7/local > /dev/null 2>&1
    ! grep -q 's7/local' "$HOME/.homebrew/trust.json" 2>/dev/null || s7_die "~/.homebrew/trust.json still names s7/local"
  fi
  rm -rf "$HOME/s7-reg" "$HOME/s7-serve"
  cp "$S7_DIR/smoke.sh" "$HOME/s7-smoke.sh" && chmod 755 "$HOME/s7-smoke.sh" || s7_die "cannot copy smoke.sh"
  grep -q HOMEBREW_CASK_OPTS "$HOME/s7-env.sh" || echo 'export HOMEBREW_CASK_OPTS="--appdir=$HOME/Applications"' >> "$HOME/s7-env.sh"
  echo "s7: this user is ready for the post-publish check (§5 step 6): . ~/s7-env.sh, one channel at a time, ~/s7-smoke.sh"
}

[ -z "${SR_S7_LIB:-}" ] || return 0 2>/dev/null || exit 0
case ${1:-} in
  prep) shift; s7_cmd_prep "$@" ;;
  brew-after) s7_cmd_brew_after ;;
  tools) s7_cmd_tools ;;
  channel) shift; s7_cmd_channel "$@" ;;
  uninstall) s7_walled user "$S7_DIR"; s7_uninstall "${2:-}" "${3:-}" || exit 1 ;;
  finish) s7_cmd_finish ;;
  *) sed -n '5,14p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
