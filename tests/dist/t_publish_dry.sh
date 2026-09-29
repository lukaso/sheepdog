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
#   - an rc tag's draft is a prerelease;
#   - __publish-dry refuses stand-ins outside /private/tmp/sd-p3-fixtures.*.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
C=$(g rev-parse "v0.1.0^{commit}"); T=$(g rev-parse v0.1.0)
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
mkout() { # dir tag commit
  rm -rf "$1"; mkdir -p "$1"
  for f in sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh; do echo "$f $1" > "$1/$f"; done
  (cd "$1" && shasum -a 256 sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh > SHA256SUMS)
  printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "signed",\n  "control": false,\n  "files": []\n}\n' "$2" "$3" > "$1/MANIFEST.json"
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
    mkdir -p "$FX/up"; cp "\$in" "$FX/up/\$nm"; echo 1 ;;
  "api repos/lukaso/sheepdog/releases/4242 --jq "*) i=0; for f in \$(ls "$FX/up"); do i=\$((i+1)); echo "\$i \$f"; done ;;
  *"releases/assets/"*) for a; do last=\$a; done; id=\${last##*/}; f=\$(ls "$FX/up" | sed -n "\${id}p"); cat "$FX/up/\$f"; [ -e "$FX/corrupt" ] && [ "\$f" = "\$(cat "$FX/corrupt")" ] && echo x ;;
  *"PATCH"*) : ;;
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
    GH_TOKEN="$DECOY" GITHUB_TOKEN="$DECOY" APPLE_APP_SPECIFIC_PASSWORD="$DECOY" NPM_TOKEN="$DECOY" \
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
  case $k in HOME|PATH|TMPDIR|USER|LOGNAME|SSH_AUTH_SOCK|GIT_SSH_COMMAND|GH_CONFIG_DIR|PWD|SHLVL|_|OLDPWD) ;; *) bad="$bad $k" ;; esac
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

fx_release 0.1.1 2 v0.1.1-rc.1; C2=$(g rev-parse "v0.1.1-rc.1^{commit}")
printf '%s\trefs/tags/v0.1.1-rc.1\n' "$C2" > "$FX/remote"
mkout "$FX/out/v0.1.1-rc.1" v0.1.1-rc.1 "$C2"
pub v0.1.1-rc.1 v0.1.1-rc.1; r=$?
[ $r = 0 ] && grep -q '^gh api -X POST repos/lukaso/sheepdog/releases -F draft=true -F prerelease=true ' "$FX/calls" && pass "an rc tag: the draft is a prerelease" || fail "rc: rc=$r $(grep POST "$FX/calls" | head -1)"

rm -f "$FX/calls"
(cd "$REPO" && env SD_PUBLISH_DRY_GH="$(command -v gh || echo /opt/homebrew/bin/gh)" SD_PUBLISH_DRY_GIT="$FX/git" sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && pass "__publish-dry refuses a gh outside the fixtures (nothing ran)" || fail "real gh accepted: rc=$r"
finish
