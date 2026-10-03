#!/bin/sh
# PHASE3.md §5: `release.sh publish-npm vTAG`, dry (`__publish-npm-dry`: npm is a stand-in given by
# path under /private/tmp/sd-p3-fixtures.*, never the real one; a fresh temp HOME). With decoy
# tokens in the environment:
#   - the whole run: for each package, platform packages first and the main one last, the npm
#     registry (registry.npmjs.org) is
#     asked whether that version is there (`view --json --prefer-online`, the registry pinned on the
#     command line), then `npm publish --access public` of a private copy of <out>/vTAG's file
#     whose bytes are the manifest's (a sibling -unsigned directory's packages never named); an rc
#     adds `--tag next` (npm makes a version without a tag `latest`); a manifest for another commit
#     than the tag's is refused;
#   - npm gets only the named environment: no decoy token reaches it;
#   - nothing at all without npm-check's stamp for this tag and manifest, or with a control manifest;
#   - each file is hashed against its manifest entry just before its upload: a package changed
#     during the run is refused there, with nothing after it published;
#   - a resumed run: a version already on npm with this file's integrity is skipped; with another
#     file's, refused, with nothing after it published; a view that fails other than with E404,
#     refused there;
#   - __publish-npm-dry refuses a stand-in outside /private/tmp/sd-p3-fixtures.*.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
PK="sheepdog-linux-arm64 sheepdog-linux-x64 sheepdog-darwin-universal sheepdog"
mkout() { # dir tag [control] [commit]: the four packages, the manifest naming each one's hash and the
           # tag's commit, npm-check's stamp
  rm -rf "$1"; mkdir -p "$1"; v=${2#v}
  for p in $PK; do echo "$p $v $1" > "$1/lukaso-$p-$v.tgz"; done
  { printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "signed",\n  "control": %s,\n  "files": [\n' "$2" "${4:-$(g rev-parse "$2^{commit}")}" "${3:-false}"
    sep=""; for p in $PK; do f=lukaso-$p-$v.tgz
      printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$f" "$(shasum -a 256 "$1/$f" | cut -d' ' -f1)"; sep=",
"; done; printf '\n  ]\n}\n'; } > "$1/MANIFEST.json"
  printf '%s %s\n' "$2" "$(shasum -a 256 "$1/MANIFEST.json" | cut -d' ' -f1)" > "$1/NPM-CHECKED"
}
integrity() { python3 -c 'import base64,hashlib,sys; print("sha512-" + base64.b64encode(hashlib.sha512(open(sys.argv[1], "rb").read()).digest()).decode())' "$1"; }
# the stand-in answers `view --json` as npm 11.6.0 was measured to (absent: exit 1 and an E404 on
# both streams; present: the integrity, quoted): from $FX/view.<package> (present), or
# $FX/viewfail.<package> (another failure); it runs $FX/onview.<package> first if there is one.
# `publish` records the sha256 of the file it was given, and runs $FX/onpublish.<package>.
cat > "$FX/npm" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.npm\\.'); env > "$FX/env.npm.\$n"
echo "npm \$*" >> "$FX/calls"
case \$1 in
  view) for a; do case \$a in @lukaso/*) p=\${a%@*}; p=\${p#@lukaso/} ;; esac; done
    [ -f "$FX/onview.\$p" ] && sh "$FX/onview.\$p"
    if [ -f "$FX/viewempty.\$p" ]; then exit 0; fi
    if [ -f "$FX/viewfail.\$p" ]; then printf '{\\n  "error": {\\n    "code": "ETIMEDOUT"\\n  }\\n}\\n'; echo "npm error code ETIMEDOUT" >&2; exit 1; fi
    if [ -f "$FX/view.\$p" ]; then printf '"%s"\\n' "\$(cat "$FX/view.\$p")"; exit 0; fi
    printf '{\\n  "error": {\\n    "code": "E404"\\n  }\\n}\\n'; echo "npm error code E404" >&2; exit 1 ;;
  publish) for a; do last=\$a; done; p=\$(basename "\$last" | sed 's/^lukaso-\\(.*\\)-[0-9][0-9.a-z-]*\\.tgz\$/\\1/')
    echo "\$p \$(shasum -a 256 "\$last" | cut -d' ' -f1) \$last" >> "$FX/pubsha"
    [ -f "$FX/onpublish.\$p" ] && sh "$FX/onpublish.\$p"; exit 0 ;;
esac
exit 1
EOF
chmod +x "$FX/npm"
pub() { # tag -> rc; output in $FX/o
  rm -f "$FX/calls" "$FX"/env.npm.* "$FX/pubsha"
  (cd "$REPO" && env HOME="$FX/ghome" NPM_TOKEN="$DECOY" NODE_AUTH_TOKEN="$DECOY" npm_config__authToken="$DECOY" GH_TOKEN="$DECOY" \
    HTTPS_PROXY=http://127.0.0.1:9 NPM_CONFIG_USERCONFIG="$FX/decoy-npmrc" \
    SD_PUBLISH_DRY_NPM="$FX/npm" sh scripts/release.sh __publish-npm-dry --out "$FX/out" "$1") > "$FX/o" 2>&1
}
# "view NAME" or "publish NAME" per call (a name's parts start with a letter, a version with a digit)
order() { sed -En 's/^npm (view|publish) .*lukaso[-\/](sheepdog(-[a-z][a-z0-9]*)*)[-@][0-9].*$/\1 \2/p' "$FX/calls" 2>/dev/null | tr '\n' ','; }
D=$FX/out/v0.1.0
mkout "$D" v0.1.0
mkout "$FX/out/v0.1.0-unsigned" v0.1.0
pub v0.1.0; r=$?
[ $r = 0 ] && pass "a dry publish-npm: done" || fail "dry publish-npm: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
[ "$(order)" = "view sheepdog-linux-arm64,publish sheepdog-linux-arm64,view sheepdog-linux-x64,publish sheepdog-linux-x64,view sheepdog-darwin-universal,publish sheepdog-darwin-universal,view sheepdog,publish sheepdog," ] \
  && pass "the order: each package asked for, then published; platform packages first, the main one last" || fail "the order: $(order)"
# each publish: the registry pinned, no --tag (a final), and a private copy of <out>/v0.1.0's file
# whose bytes, when npm got it, were the manifest's
REG="--registry=https://registry.npmjs.org/ --@lukaso:registry=https://registry.npmjs.org/"
bad=""; for p in $PK; do
  l=$(grep "^npm publish .*/lukaso-$p-0.1.0.tgz\$" "$FX/calls"); f=${l##* }
  case $l in "npm publish --access public $REG /private/tmp/sd-npmpub."*"/lukaso-$p-0.1.0.tgz") ;; *) bad="$bad [$l]" ;; esac
  [ "$(awk -v p="$p" '$1 == p {print $2}' "$FX/pubsha")" = "$(shasum -a 256 "$D/lukaso-$p-0.1.0.tgz" | cut -d' ' -f1)" ] || bad="$bad [$p: not the manifest's bytes]"
done
[ -z "$bad" ] && pass "each publish: the registry pinned, no --tag (a final), a private copy holding the manifest's bytes" || fail "publish lines:$bad"
[ "$(grep -c "^npm view --json --prefer-online $REG @lukaso/sheepdog[a-z0-9-]*@0.1.0 dist.integrity\$" "$FX/calls")" = 4 ] \
  && pass "each view: --json, --prefer-online (not npm's cache), the registry pinned" || fail "view lines: $(grep '^npm view' "$FX/calls" | head -1)"
grep -q 'unsigned' "$FX/calls" && fail "a call names the -unsigned sibling" || pass "the -unsigned sibling's packages are never named"
pd=$(awk 'NR == 1 {print $3}' "$FX/pubsha"); pd=${pd%/*}
case $pd in /private/tmp/sd-npmpub.*) [ ! -e "$pd" ] && pass "this run's private copy dir is gone" || fail "$pd is left" ;; *) fail "no private copy dir recorded: '$pd'" ;; esac
# npm's environment class (npm_env, the real run's too): only the named variables; a proxy reaches
# it (the control), a token or an npmrc of the caller's never does in the dry run
bad=""; for f in "$FX"/env.npm.*; do for k in $(sed 's/=.*//' "$f"); do
  case $k in HOME|PATH|TMPDIR|USER|LOGNAME|PWD|SHLVL|_|OLDPWD|HTTPS_PROXY) ;; *) bad="$bad $k" ;; esac
done; done
[ -z "$bad" ] && pass "npm got only the named environment" || fail "extra environment:$(printf '%s\n' $bad | sort -u | tr '\n' ' ')"
grep -q '^HTTPS_PROXY=http://127.0.0.1:9$' "$FX/env.npm.0" && pass "control: the proxy reaches npm" || fail "control: no HTTPS_PROXY for npm"
grep -rl "$DECOY" "$FX"/env.npm.* "$FX/calls" "$FX/o" >/dev/null 2>&1 && fail "a decoy token reached npm or the output" || pass "no decoy token reached npm or the output"
h=$(sed -n 's/^HOME=//p' "$FX/env.npm.0"); case $h in /private/tmp/sd-dryhome.*) pass "the dry npm's HOME is a fresh temp dir" ;; *) fail "the dry npm's HOME is '$h'" ;; esac

# every package is checked before the first npm call: one changed before the run means none is published
mkout "$D" v0.1.0; echo x >> "$D/lukaso-sheepdog-0.1.0.tgz"; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && pass "a package changed after npm-check, before the run: refused, npm never called" || fail "changed before the run: rc=$r $(order)"
# a view that answers nothing (exit 0, no integrity) cannot say the version is absent
mkout "$D" v0.1.0; : > "$FX/viewempty.sheepdog-linux-arm64"; pub v0.1.0; r=$?; rm -f "$FX/viewempty.sheepdog-linux-arm64"
[ $r = 1 ] && ! grep -q '^npm publish' "$FX/calls" && grep -q 'cannot tell' "$FX/o" && pass "a view with no answer: refused, nothing published" || fail "empty view: rc=$r $(order)"
# a refused run leaves its private copy dir behind neither
mkout "$D" v0.1.0; : > "$FX/viewfail.sheepdog-darwin-universal"; pub v0.1.0; rm -f "$FX/viewfail.sheepdog-darwin-universal"
pd=$(awk 'NR == 1 {print $3}' "$FX/pubsha"); pd=${pd%/*}
case $pd in /private/tmp/sd-npmpub.*) [ ! -e "$pd" ] && pass "a refused run's private copy dir is gone" || fail "$pd is left after a refusal" ;; *) fail "no private copy dir recorded after the refusal: '$pd'" ;; esac

# nothing at all without the stamp, with a stamp for another manifest, or with a control manifest
rm "$D/NPM-CHECKED"; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && grep -q npm-check "$FX/o" && pass "no npm-check stamp: refused, npm never called" || fail "no stamp: rc=$r $(order)"
mkout "$D" v0.1.0; echo ' ' >> "$D/MANIFEST.json"; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && pass "a stamp for another manifest: refused, npm never called" || fail "stale stamp: rc=$r $(order)"
mkout "$D" v0.1.0 true; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && grep -q control "$FX/o" && pass "a control manifest (stamped): refused, npm never called" || fail "control: rc=$r $(order)"
mkout "$D" v0.1.0 false 0000000000000000000000000000000000000000; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && grep -q commit "$FX/o" && pass "a manifest for another commit than the tag's (stamped): refused, npm never called" || fail "other commit: rc=$r $(order)"
# the bytes published are the bytes hashed: a source changed after its check (by the view) is not
# what npm gets
mkout "$D" v0.1.0; echo "echo x >> '$D/lukaso-sheepdog-darwin-universal-0.1.0.tgz'" > "$FX/onview.sheepdog-darwin-universal"
h0=$(sed -n 's/.*"lukaso-sheepdog-darwin-universal-0.1.0.tgz", "sha256": "\([0-9a-f]*\)".*/\1/p' "$D/MANIFEST.json")
pub v0.1.0; r=$?; rm -f "$FX/onview.sheepdog-darwin-universal"
[ $r = 0 ] && [ "$(awk '$1 == "sheepdog-darwin-universal" {print $2}' "$FX/pubsha")" = "$h0" ] \
  && pass "a source changed between its check and its upload: npm got the checked bytes" || fail "changed after the check: rc=$r $(cat "$FX/pubsha" 2>/dev/null | tr '\n' ' ')"
# a view that fails for another reason than E404 cannot say the version is absent: refused there
mkout "$D" v0.1.0; : > "$FX/viewfail.sheepdog-darwin-universal"; pub v0.1.0; r=$?; rm -f "$FX/viewfail.sheepdog-darwin-universal"
[ $r = 1 ] && [ "$(order)" = "view sheepdog-linux-arm64,publish sheepdog-linux-arm64,view sheepdog-linux-x64,publish sheepdog-linux-x64,view sheepdog-darwin-universal," ] && grep -q 'cannot tell' "$FX/o" \
  && pass "a view that fails (not E404): refused there, nothing after it published" || fail "view failure: rc=$r $(order) $(tail -1 "$FX/o")"

# a package changed during the run (by the first upload, as anything else could): refused at it
mkout "$D" v0.1.0; echo "echo x >> '$D/lukaso-sheepdog-darwin-universal-0.1.0.tgz'" > "$FX/onpublish.sheepdog-linux-arm64"
pub v0.1.0; r=$?; rm -f "$FX/onpublish.sheepdog-linux-arm64"
[ $r = 1 ] && [ "$(order)" = "view sheepdog-linux-arm64,publish sheepdog-linux-arm64,view sheepdog-linux-x64,publish sheepdog-linux-x64," ] \
  && grep -q 'lukaso-sheepdog-darwin-universal-0.1.0.tgz does not match its manifest hash' "$FX/o" \
  && pass "a package changed during the run: refused just before its upload, nothing after it published" || fail "changed mid-run: rc=$r $(order) $(tail -1 "$FX/o")"

# a resumed run: the same file already there is skipped; another file there stops the run
mkout "$D" v0.1.0; integrity "$D/lukaso-sheepdog-linux-arm64-0.1.0.tgz" > "$FX/view.sheepdog-linux-arm64"
pub v0.1.0; r=$?
[ $r = 0 ] && ! grep -q '^npm publish .*linux-arm64' "$FX/calls" && [ "$(grep -c '^npm publish' "$FX/calls")" = 3 ] && grep -q 'already on npm (the same file)' "$FX/o" \
  && pass "a version already on npm with this file: skipped, the rest published" || fail "resume same: rc=$r $(order)"
echo "sha512-AAAA" > "$FX/view.sheepdog-linux-x64"
pub v0.1.0; r=$?; rm -f "$FX"/view.*
[ $r = 1 ] && [ "$(order)" = "view sheepdog-linux-arm64,view sheepdog-linux-x64," ] && grep -q 'another file' "$FX/o" \
  && pass "a version already on npm with another file: refused there, nothing after it published" || fail "resume other: rc=$r $(order) $(tail -1 "$FX/o")"

# an rc: every publish carries --tag next
fx_release 0.1.1 2 v0.1.1-rc.1
mkout "$FX/out/v0.1.1-rc.1" v0.1.1-rc.1
pub v0.1.1-rc.1; r=$?
[ $r = 0 ] && [ "$(grep -c "^npm publish --access public $REG --tag next " "$FX/calls")" = 4 ] && pass "an rc: each publish has --tag next" || fail "rc: rc=$r $(grep '^npm publish' "$FX/calls" | head -1)"

# __publish-npm-dry refuses a stand-in outside the fixtures; the outside one is never called
OUT=$(mktemp -d /private/tmp/sd-outside.XXXXXX); trap 'rm -rf "$OUT" "$FX"' EXIT
printf '#!/bin/sh\necho "OUTSIDE $*" >> "%s/called"\nexit 0\n' "$OUT" > "$OUT/npm"; chmod +x "$OUT/npm"
ln -s "$OUT/npm" "$FX/npm-link"
for spec in "outside:$OUT/npm" "symlink:$FX/npm-link"; do
  l=${spec%%:*} p=${spec#*:}; rm -f "$OUT/called"
  (cd "$REPO" && env HOME="$FX/ghome" SD_PUBLISH_DRY_NPM="$p" sh scripts/release.sh __publish-npm-dry --out "$FX/out" v0.1.0) > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && [ ! -e "$OUT/called" ] && pass "__publish-npm-dry, a $l npm: refused, the outside npm never called" || fail "__publish-npm-dry, $l: rc=$r"
done
n=$(ls -d /private/tmp/sd-dryhome.* 2>/dev/null | wc -l | tr -d ' ')
[ "$n" = 0 ] && pass "no dry HOME is left, after the refused runs too" || fail "$n dry HOME dirs left"
finish
