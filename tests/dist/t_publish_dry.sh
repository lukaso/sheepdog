#!/bin/sh
# PHASE3.md §1.2: release.sh's publish executor, dry (`__publish-dry`: gh and git are stand-ins
# given by path under /private/tmp/sd-p3-fixtures.*, never the real ones; verify is not run; the
# confirmations come from a script). In the operator's locale (it sorts SHA256SUMS after install.sh;
# release.sh sets LC_ALL=C) and with decoy tokens in the environment:
#   - the whole run: the remote tag read from github.com/lukaso/sheepdog itself, the releases
#     listed, the draft made, five uploads, the draft read back, five downloads compared, the tag
#     read again, the second confirmation, then the PATCH;
#   - gh and git get only the named environment: no decoy token reaches them;
#   - no PATCH when a download differs, when the tag moved, or when the second answer is wrong;
#   - nothing at all without npm-check's stamp for this tag and this manifest, or when a file it
#     checked (an npm package included) is changed or gone; a file changed during its own upload
#     is refused by the manifest's hash;
#   - a PATCH that fails: the release read again, and its real state said (public: 0; a draft or
#     unreadable: 1);
#   - an rc tag's draft is a prerelease;
#   - __publish-dry refuses stand-ins outside /private/tmp/sd-p3-fixtures.*.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
C=$(g rev-parse "v0.1.0^{commit}"); T=$(g rev-parse v0.1.0)
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
mkout() { # dir tag commit: the five files, the manifest naming each one's hash, and npm-check's
           # stamp for that manifest (as a passed npm-check leaves it)
  rm -rf "$1"; mkdir -p "$1"; v=${2#v}
  for f in sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh \
           lukaso-sheepdog-$v.tgz lukaso-sheepdog-darwin-universal-$v.tgz lukaso-sheepdog-linux-arm64-$v.tgz lukaso-sheepdog-linux-x64-$v.tgz; do echo "$f $1" > "$1/$f"; done
  (cd "$1" && shasum -a 256 sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh > SHA256SUMS)
  { printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "signed",\n  "control": false,\n  "files": [\n' "$2" "$3"
    sep=""; for f in sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh \
                     lukaso-sheepdog-$v.tgz lukaso-sheepdog-darwin-universal-$v.tgz lukaso-sheepdog-linux-arm64-$v.tgz lukaso-sheepdog-linux-x64-$v.tgz; do
      printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$f" "$(shasum -a 256 "$1/$f" | cut -d' ' -f1)"; sep=",
"; done; printf '\n  ]\n}\n'; } > "$1/MANIFEST.json"
  printf '%s %s\n' "$2" "$(shasum -a 256 "$1/MANIFEST.json" | cut -d' ' -f1)" > "$1/NPM-CHECKED"
}
# the stand-ins
cat > "$FX/gh" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.gh\\.'); env > "$FX/env.gh.\$n"
echo "gh \$*" >> "$FX/calls"
case "\$*" in
  *"releases --paginate"*) cat "$FX/existing" 2>/dev/null ;;
  "api -X POST repos/lukaso/sheepdog/releases "*) echo 4242 ;;
  *"uploads.github.com"*) prev=""; for a; do case \$a in *assets\\?name=*) nm=\${a##*name=} ;; esac; [ "\$prev" = --input ] && in=\$a; prev=\$a; done
    [ -e "$FX/mutate" ] && [ "\$nm" = "\$(cat "$FX/mutate")" ] && echo changed >> "\$in"
    mkdir -p "$FX/up"; cp "\$in" "$FX/up/\$nm"; echo 1 ;;
  "api repos/lukaso/sheepdog/releases/4242 --jq .assets"*) i=0; for f in \$(ls "$FX/up"); do i=\$((i+1)); echo "\$i \$f"; done ;;
  *"releases/assets/"*) for a; do last=\$a; done; id=\${last##*/}; f=\$(ls "$FX/up" | sed -n "\${id}p"); cat "$FX/up/\$f"; [ -e "$FX/corrupt" ] && [ "\$f" = "\$(cat "$FX/corrupt")" ] && echo x ;;
  *"PATCH"*) [ -e "$FX/patchfail" ] && exit 1 ;;
  *"--jq .draft"*) s=\$(cat "$FX/draftstate" 2>/dev/null); [ "\$s" = fail ] && exit 1; echo "\$s" ;;
esac
exit 0
EOF
cat > "$FX/git" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.git\\.'); env > "$FX/env.git.\$n"
echo "git \$*" >> "$FX/calls"
if [ \$n -ge 1 ] && [ -e "$FX/moved" ]; then cat "$FX/moved"; else cat "$FX/remote"; fi
EOF
chmod +x "$FX/gh" "$FX/git"
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"
pub() { # tag answers -> rc; output in $FX/o
  rm -rf "$FX/calls" "$FX"/env.* "$FX/up"; printf '%s\n' "$2" > "$FX/ask"
  (cd "$REPO" && env -u LC_ALL LANG=en_GB.UTF-8 LC_COLLATE=en_GB.UTF-8 HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    GH_TOKEN="$DECOY" GITHUB_TOKEN="$DECOY" APPLE_APP_SPECIFIC_PASSWORD="$DECOY" NPM_TOKEN="$DECOY" GH_CONFIG_DIR="$FX/ghcfg" SSH_AUTH_SOCK="$FX/sock" \
    SD_PUBLISH_DRY_GH="$FX/gh" SD_PUBLISH_DRY_GIT="$FX/git" SD_ASK_SCRIPT="$FX/ask" SD_ASK_RECORD="$FX/calls" \
    sh scripts/release.sh __publish-dry --out "$FX/out" "$1") > "$FX/o" 2>&1
}
seq() { sed -e 's/^gh api -X POST repos.*/POST/; s/^gh api -X POST -H .*/UPLOAD/; s/^gh api repos\/lukaso\/sheepdog\/releases --paginate.*/LIST/' \
  -e 's/^gh api repos\/lukaso\/sheepdog\/releases\/4242 .*/READ/; s/^gh api -H .*assets.*/DL/; s/^gh api -X PATCH.*/PATCH/' \
  -e 's/^git ls-remote .*/LSREMOTE/; s/^ask .*/ASK/' "$FX/calls" | tr '\n' ' '; }

mkout "$FX/out/v0.1.0" v0.1.0 "$C"
pub v0.1.0 v0.1.0; r=$?
[ $r = 0 ] && pass "a dry publish in the operator's locale: done" || fail "dry publish: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
[ "$(seq)" = "LSREMOTE LIST POST UPLOAD UPLOAD UPLOAD UPLOAD UPLOAD READ DL DL DL DL DL LSREMOTE ASK PATCH " ] \
  && pass "the order: tag, list, draft, 5 uploads, read, 5 downloads, tag again, confirm, publish" || fail "the order: $(seq)"
[ "$(grep -c '^git ls-remote https://github.com/lukaso/sheepdog refs/tags/v0.1.0\*$' "$FX/calls")" = 2 ] \
  && pass "the tag is read from github.com/lukaso/sheepdog itself (twice)" || fail "ls-remote: $(grep ls-remote "$FX/calls" | head -1)"
bad=""; for f in "$FX"/env.*; do for k in $(sed 's/=.*//' "$f"); do
  case $k in HOME|PATH|TMPDIR|USER|LOGNAME|PWD|SHLVL|_|OLDPWD) ;; *) bad="$bad $k" ;; esac
done; done
[ -z "$bad" ] && pass "gh and git got only the named environment" || fail "extra environment:$(printf '%s\n' $bad | sort -u | tr '\n' ' ')"
grep -rl "$DECOY" "$FX"/env.* "$FX/calls" "$FX/o" >/dev/null 2>&1 && fail "a decoy token reached gh, git or the output" || pass "no decoy token reached gh, git or the output"

echo sheepdog-linux-x86_64 > "$FX/corrupt"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/corrupt"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "a download that differs: no PATCH (1)" || fail "corrupt download: rc=$r"
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" 0000000000000000000000000000000000000000 > "$FX/moved"
pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/moved"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "the tag moved during the upload: no PATCH (1)" || fail "moved tag: rc=$r"
pub v0.1.0 no; r=$?
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "a wrong second answer: no PATCH (1)" || fail "wrong answer: rc=$r"

# a file changed during its own upload (the upload and the local file agree, the manifest does
# not): the downloads are compared with the manifest, so no PATCH
echo sheepdog-linux-aarch64 > "$FX/mutate"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/mutate"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && grep -q 'manifest' "$FX/o" && pass "a file changed during its upload: no PATCH, the manifest named (1)" || fail "changed during upload: rc=$r $(tail -1 "$FX/o")"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# every file npm-check checked is still the manifest's: an npm package changed or gone after it
# is refused before any call
for x in change rm; do
  mkout "$FX/out/v0.1.0" v0.1.0 "$C"
  if [ $x = change ]; then echo x >> "$FX/out/v0.1.0/lukaso-sheepdog-darwin-universal-0.1.0.tgz"; else rm "$FX/out/v0.1.0/lukaso-sheepdog-0.1.0.tgz"; fi
  pub v0.1.0 v0.1.0; r=$?
  [ $r = 1 ] && [ ! -s "$FX/calls" ] && pass "an npm package $( [ $x = change ] && echo changed || echo removed) after npm-check: refused before any call" || fail "npm package $x: rc=$r calls=$(seq)"
done
mkout "$FX/out/v0.1.0" v0.1.0 "$C"

# npm-check comes first: without its stamp for this manifest, nothing reaches GitHub
rm "$FX/out/v0.1.0/NPM-CHECKED"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q 'npm-check' "$FX/o" && pass "no npm-check stamp: refused before any call, npm-check named" || fail "no stamp: rc=$r calls=$(seq)"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; echo ' ' >> "$FX/out/v0.1.0/MANIFEST.json"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && pass "a stamp for another manifest (changed after npm-check): refused before any call" || fail "stale stamp: rc=$r calls=$(seq)"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; sed 's/^v0.1.0 /v0.1.0-rc.9 /' "$FX/out/v0.1.0/NPM-CHECKED" > "$FX/s" && mv "$FX/s" "$FX/out/v0.1.0/NPM-CHECKED"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && pass "a stamp for another tag: refused before any call" || fail "other tag's stamp: rc=$r calls=$(seq)"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# the last request fails: the release is read again and its real state reported
touch "$FX/patchfail"
echo false > "$FX/draftstate"; pub v0.1.0 v0.1.0; r=$?
[ $r = 0 ] && grep -q 'v0.1.0 is public' "$FX/o" && pass "the PATCH failed but the release is public: said so (0)" || fail "patch failed, public: rc=$r $(tail -1 "$FX/o")"
echo true > "$FX/draftstate"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && grep -q 'still a draft' "$FX/o" && grep -q 'draft 4242' "$FX/o" && grep -q 'delete' "$FX/o" && pass "the PATCH failed and the release is still a draft: said so, with the draft and the way on (1)" || fail "patch failed, draft: rc=$r $(tail -1 "$FX/o")"
echo fail > "$FX/draftstate"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && grep -q 'check it on GitHub' "$FX/o" && pass "the PATCH failed and the state cannot be read: said so (1)" || fail "patch failed, unreadable: rc=$r $(tail -1 "$FX/o")"
rm -f "$FX/patchfail" "$FX/draftstate"

fx_release 0.1.1 2 v0.1.1-rc.1; C2=$(g rev-parse "v0.1.1-rc.1^{commit}")
printf '%s\trefs/tags/v0.1.1-rc.1\n' "$C2" > "$FX/remote"
mkout "$FX/out/v0.1.1-rc.1" v0.1.1-rc.1 "$C2"
pub v0.1.1-rc.1 v0.1.1-rc.1; r=$?
[ $r = 0 ] && grep -q '^gh api -X POST repos/lukaso/sheepdog/releases -F draft=true -F prerelease=true ' "$FX/calls" && pass "an rc tag: the draft is a prerelease" || fail "rc: rc=$r $(grep POST "$FX/calls" | head -1)"

# __publish-dry refuses every way to reach a gh outside the fixtures. The "real gh" here is a
# recording stand-in outside them (never the real one), run with a temp HOME; it must never be called.
OUT=$(mktemp -d /private/tmp/sd-outside.XXXXXX); trap 'rm -rf "$OUT" "$FX"' EXIT
printf '#!/bin/sh\necho "OUTSIDE $*" >> "%s/called"\nenv > "%s/env"\nexit 0\n' "$OUT" "$OUT" > "$OUT/gh"; chmod +x "$OUT/gh"
ln -s "$OUT/gh" "$FX/gh-link"
printf '#!/bin/sh\nexec "%s" "$@"\n' "$OUT/gh" > "$FX/gh-wrap"; chmod +x "$FX/gh-wrap"
for spec in "outside:$OUT/gh" "symlink:$FX/gh-link" "dotdot:$FX/../$(basename "$OUT")/gh"; do
  l=${spec%%:*} p=${spec#*:}; rm -f "$OUT/called"
  (cd "$REPO" && env HOME="$FX/ghome" SD_PUBLISH_DRY_GH="$p" SD_PUBLISH_DRY_GIT="$FX/git" SD_ASK_SCRIPT="$FX/ask" SD_ASK_RECORD="$FX/calls2" \
    sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && [ ! -e "$OUT/called" ] && pass "__publish-dry, a $l gh: refused, the outside gh never called" || fail "__publish-dry, $l: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1)"
done
# a stand-in whose name ends in a newline, next to a symlink without it: refused, the symlink's
# target never called (the checked name and the run name must be one string)
nl='
'
cp "$FX/gh" "$FX/ghn$nl"; ln -s "$OUT/gh" "$FX/ghn"; rm -f "$OUT/called"
(cd "$REPO" && env HOME="$FX/ghome" SD_PUBLISH_DRY_GH="$FX/ghn$nl" SD_PUBLISH_DRY_GIT="$FX/git" SD_ASK_SCRIPT="$FX/ask" SD_ASK_RECORD="$FX/calls2" \
  sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
[ $r != 0 ] && [ ! -e "$OUT/called" ] && pass "a stand-in name ending in a newline: refused, the symlink's target never called" || fail "newline name: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1)"
# a wrapper inside the fixtures that execs a gh outside them cannot be seen before it runs: what
# it reaches runs with a fresh temp HOME and no token or transport variable, so a real gh would
# have no login and could not write
rm -f "$OUT/called" "$OUT/env"
(cd "$REPO" && env HOME="$FX/ghome" GH_TOKEN="$DECOY" GH_CONFIG_DIR="$FX/ghcfg" SSH_AUTH_SOCK="$FX/sock" SD_PUBLISH_DRY_GH="$FX/gh-wrap" \
  SD_PUBLISH_DRY_GIT="$FX/git" SD_ASK_SCRIPT="$FX/ask" SD_ASK_RECORD="$FX/calls2" sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1
if [ -e "$OUT/env" ]; then
  h=$(sed -n 's/^HOME=//p' "$OUT/env")
  case $h in /private/tmp/sd-*) ! grep -q -e '^GH_TOKEN=' -e '^GH_CONFIG_DIR=' -e '^SSH_AUTH_SOCK=' -e '^GITHUB_TOKEN=' "$OUT/env" \
      && pass "a wrapper reaching an outside gh: that gh ran with a temp HOME and no token or transport" || fail "wrapper: the outside gh got a token or transport" ;;
    *) fail "wrapper: the outside gh's HOME is '$h'" ;; esac
else pass "a wrapper reaching an outside gh: not reached"; fi
# the dry run's gh and git get a fresh temp HOME, and no GH_CONFIG_DIR or SSH_AUTH_SOCK: even a
# real gh that got this far would have no login
pub v0.1.1-rc.1 v0.1.1-rc.1
h=$(sed -n 's/^HOME=//p' "$FX/env.gh.0"); case $h in /private/tmp/sd-*) pass "the dry gh's HOME is a fresh temp dir ($h)" ;; *) fail "the dry gh's HOME is '$h'" ;; esac
grep -q -e '^GH_CONFIG_DIR=' -e '^SSH_AUTH_SOCK=' "$FX"/env.gh.* "$FX"/env.git.* && fail "the dry gh or git got GH_CONFIG_DIR or SSH_AUTH_SOCK" || pass "the dry gh and git get no GH_CONFIG_DIR or SSH_AUTH_SOCK"
n=$(ls -d /private/tmp/sd-dryhome.* 2>/dev/null | wc -l | tr -d ' ')
[ "$n" = 0 ] && pass "no dry HOME is left, after the refused runs too" || fail "$n dry HOME dirs left"
finish
