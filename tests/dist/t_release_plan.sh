#!/bin/sh
# PHASE3.md §1.2: scripts/release-plan.sh, the publish planner. It is pure: it reads the release's
# output directory, the local tag, the remote tag (a `git ls-remote` output given as a file) and
# the releases that exist (tag names given as a file), and prints the exact GitHub calls, or
# refuses (exit 1). `release-plan.sh --validate PLAN` refuses a plan that is not of the one shape.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
C=$(g rev-parse "v0.1.0^{commit}"); T=$(g rev-parse v0.1.0)
P="$SD_ROOT/scripts/release-plan.sh"
FIVE="sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh SHA256SUMS"
mkout() { # dir [mode control]
  rm -rf "$1"; mkdir -p "$1"
  for f in sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh; do echo "$f $1" > "$1/$f"; done
  (cd "$1" && shasum -a 256 sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh > SHA256SUMS)
  printf '{\n  "v": 1,\n  "tag": "v0.1.0",\n  "commit": "%s",\n  "mode": "%s",\n  "control": %s,\n  "files": []\n}\n' "$C" "${2:-signed}" "${3:-false}" > "$1/MANIFEST.json"
}
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"
: > "$FX/releases"
plan() { # dir [remote] [releases] -> rc, plan in $FX/plan
  (cd "$REPO" && env HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    sh "$P" --out "$1" --tag v0.1.0 --remote "${2:-$FX/remote}" --releases "${3:-$FX/releases}") > "$FX/plan" 2>"$FX/err"
}
ok() { plan "$@"; r=$?; [ $r = 0 ]; }
no() { plan "$@"; r=$?; [ $r = 1 ]; }

mkout "$FX/o"
if ok "$FX/o"; then pass "a good release plans"; else fail "a good release: rc=$r $(cat "$FX/err")"; fi
grep -q '^POST repos/lukaso/sheepdog/releases -F draft=true -f tag_name=v0.1.0 -f name=v0.1.0$' "$FX/plan" && pass "POST: draft true (a boolean), the tag, no target_commitish" || fail "POST line: $(grep POST "$FX/plan")"
[ "$(grep -c '^UPLOAD ' "$FX/plan")" = 5 ] && pass "5 uploads" || fail "uploads: $(grep -c '^UPLOAD ' "$FX/plan")"
for f in $FIVE; do grep -qx "UPLOAD $f" "$FX/plan" || fail "no upload of $f"; done
grep -qx 'PATCH -F draft=false' "$FX/plan" && pass "PATCH: only draft false" || fail "PATCH line: $(grep PATCH "$FX/plan")"
sh "$P" --validate "$FX/plan" && pass "its own plan validates" || fail "its own plan does not validate"

mkout "$FX/o"; echo x > "$FX/o/notary-profile.txt"; echo x > "$FX/o/lukaso-sheepdog-0.1.0.tgz"
ok "$FX/o" && ! grep -q -e notary -e tgz "$FX/plan" && pass "other files in the directory: allowed, never uploaded" || fail "other files: rc=$r"
for f in $FIVE; do
  mkout "$FX/o"; rm "$FX/o/$f"
  no "$FX/o" && pass "missing $f: refused" || fail "missing $f: rc=$r"
done
mkout "$FX/o"; echo changed >> "$FX/o/sheepdog-linux-aarch64"
no "$FX/o" && pass "a file not matching SHA256SUMS: refused" || fail "hash mismatch: rc=$r"
mkout "$FX/o"; echo "0000  sheepdog-linux-aarch64" >> "$FX/o/SHA256SUMS"
no "$FX/o" && pass "a malformed or fifth SHA256SUMS line: refused" || fail "SHA256SUMS lines: rc=$r"
mkout "$FX/o"; (cd "$FX/o" && shasum -a 256 sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh > SHA256SUMS)
no "$FX/o" && pass "a SHA256SUMS that misses an artifact: refused" || fail "short SHA256SUMS: rc=$r"
mkout "$FX/o" signed true; no "$FX/o" && pass "a control build (mode signed, control true): refused" || fail "control: rc=$r"
mkout "$FX/o"; (cd "$FX/o" && shasum -a 256 sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-aarch64 sheepdog-linux-x86_64 > SHA256SUMS)
no "$FX/o" && pass "four matching lines that name a file twice (install.sh missing): refused" || fail "duplicate name: rc=$r"
mkout "$FX/o" unsigned false; no "$FX/o" && pass "an unsigned build: refused" || fail "unsigned: rc=$r"
mkout "$FX/o"; sed -i.b "s/$C/0000000000000000000000000000000000000000/" "$FX/o/MANIFEST.json"
no "$FX/o" && pass "a manifest commit that is not the tag's: refused" || fail "manifest commit: rc=$r"
mkout "$FX/o"
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" 0000000000000000000000000000000000000000 > "$FX/r2"
no "$FX/o" "$FX/r2" && pass "a remote tag on another commit: refused" || fail "remote other commit: rc=$r"
printf '%s\trefs/tags/v0.1.0\n' "$T" > "$FX/r3"
no "$FX/o" "$FX/r3" && pass "an annotated remote tag without its peeled line: refused (the tag object is not the commit)" || fail "unpeeled: rc=$r"
: > "$FX/r4"; no "$FX/o" "$FX/r4" && pass "no remote tag: refused" || fail "no remote tag: rc=$r"
printf '%s\trefs/tags/v0.1.0\n' "$C" > "$FX/r5"
ok "$FX/o" "$FX/r5" && pass "a lightweight remote tag on the commit: accepted" || fail "lightweight: rc=$r"
echo v0.1.0 > "$FX/rel2"; no "$FX/o" "$FX/remote" "$FX/rel2" && pass "a release or draft that already uses the tag: refused" || fail "existing release: rc=$r"

# --validate refuses every other shape
ok "$FX/o"; cp "$FX/plan" "$FX/good"
bad() { # label sed-expression
  sed "$2" "$FX/good" > "$FX/bad"
  cmp -s "$FX/bad" "$FX/good" && { fail "$1: the change was not made"; return; }
  sh "$P" --validate "$FX/bad" >/dev/null 2>&1 && fail "$1: validated" || pass "$1: refused"
}
bad "draft sent as a string (-f)" 's/-F draft=true/-f draft=true/'
bad "a target_commitish" 's/-f name=v0.1.0$/-f name=v0.1.0 -f target_commitish=main/'
bad "a PATCH that names a tag" 's/^PATCH -F draft=false$/PATCH -F draft=false -f tag_name=v9/'
bad "a PATCH that keeps the draft" 's/^PATCH -F draft=false$/PATCH -F draft=true/'
bad "a sixth upload" '$a\
UPLOAD MANIFEST.json'
bad "a missing PATCH" '/^PATCH/d'
finish
