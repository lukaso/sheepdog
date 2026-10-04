#!/bin/sh
# PHASE3.md §5: `release.sh publish-npm vTAG`, dry (`__publish-npm-dry`: npm is a stand-in given by
# path under /private/tmp/sr-p3-fixtures.*, never the real one; a fresh temp HOME). With decoy
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
#   - __publish-npm-dry refuses a stand-in outside /private/tmp/sr-p3-fixtures.*.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
PK="sheepr-linux-arm64 sheepr-linux-x64 sheepr-darwin-universal sheepr"
mkout() { # dir tag [control] [commit]: the four packages, the manifest naming each one's hash and the
           # tag's commit, npm-check's stamp
  rm -rf "$1"; mkdir -p "$1"; v=${2#v}
  for p in $PK; do echo "$p $v $1" > "$1/$p-$v.tgz"; done
  { printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "signed",\n  "control": %s,\n  "files": [\n' "$2" "${4:-$(g rev-parse "$2^{commit}")}" "${3:-false}"
    sep=""; for p in $PK; do f=$p-$v.tgz
      printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$f" "$(shasum -a 256 "$1/$f" | cut -d' ' -f1)"; sep=",
"; done; printf '\n  ]\n}\n'; } > "$1/MANIFEST.json"
  printf '%s %s\n' "$2" "$(shasum -a 256 "$1/MANIFEST.json" | cut -d' ' -f1)" > "$1/NPM-CHECKED"
}
integrity() { python3 -c 'import base64,hashlib,sys; print("sha512-" + base64.b64encode(hashlib.sha512(open(sys.argv[1], "rb").read()).digest()).decode())' "$1"; }
# `whoami` prints $FX/whoami (exit 1 with ENEEDAUTH if $FX/whoamifail exists); `owner ls NAME`
# prints a `user <email>` line per line of $FX/owner.NAME (default: lukasco), or exits 1 with E404
# ($FX/ownerfail.NAME) or another error ($FX/ownerbad.NAME), as npm 11.6.0 was measured to.
# the stand-in answers `view --json` as npm 11.6.0 was measured to (absent: exit 1 and an E404 on
# both streams; present: the integrity, quoted): from $FX/view.<package> (present), or
# $FX/viewfail.<package> (another failure); it runs $FX/onview.<package> first if there is one.
# `publish` records the sha256 of the file it was given, and runs $FX/onpublish.<package>.
cat > "$FX/npm" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.npm\\.'); env > "$FX/env.npm.\$n"
echo "npm \$*" >> "$FX/calls"
case \$1 in
  view) for a; do case \$a in sheepr*@*) p=\${a%@*} ;; esac; done
    [ -f "$FX/onview.\$p" ] && sh "$FX/onview.\$p"
    if [ -f "$FX/viewempty.\$p" ]; then exit 0; fi
    if [ -f "$FX/viewfail.\$p" ]; then printf '{\\n  "error": {\\n    "code": "ETIMEDOUT"\\n  }\\n}\\n'; echo "npm error code ETIMEDOUT" >&2; exit 1; fi
    if [ -f "$FX/view.\$p" ]; then printf '"%s"\\n' "\$(cat "$FX/view.\$p")"; exit 0; fi
    printf '{\\n  "error": {\\n    "code": "E404"\\n  }\\n}\\n'; echo "npm error code E404" >&2; exit 1 ;;
  whoami) if [ -f "$FX/whoamifail" ]; then echo "npm error code ENEEDAUTH" >&2; exit 1; fi; cat "$FX/whoami"; exit 0 ;;
  owner) p=\$3
    if [ -f "$FX/ownerfail.\$p" ]; then echo "npm error owner ls Couldn't get owner data \$p" >&2; echo "npm error code E404" >&2; exit 1; fi
    if [ -f "$FX/ownerbad.\$p" ]; then echo "npm error code ETIMEDOUT" >&2; exit 1; fi
    if [ -f "$FX/owner.\$p" ]; then while read -r u; do echo "\$u <\$u@example.com>"; done < "$FX/owner.\$p"; else echo "lukasco <lukasco@example.com>"; fi; exit 0 ;;
  publish) for a; do last=\$a; done; p=\$(basename "\$last" | sed 's/^\\(.*\\)-[0-9][0-9.a-z-]*\\.tgz\$/\\1/')
    echo "\$p \$(shasum -a 256 "\$last" | cut -d' ' -f1) \$last" >> "$FX/pubsha"
    [ -f "$FX/onpublish.\$p" ] && sh "$FX/onpublish.\$p"; exit 0 ;;
esac
exit 1
EOF
chmod +x "$FX/npm"
echo lukasco > "$FX/whoami"
pub() { # tag -> rc; output in $FX/o
  rm -f "$FX/calls" "$FX"/env.npm.* "$FX/pubsha"
  (cd "$REPO" && env HOME="$FX/ghome" NPM_TOKEN="$DECOY" NODE_AUTH_TOKEN="$DECOY" npm_config__authToken="$DECOY" GH_TOKEN="$DECOY" \
    HTTPS_PROXY=http://127.0.0.1:9 NPM_CONFIG_USERCONFIG="$FX/decoy-npmrc" \
    SR_PUBLISH_DRY_NPM="$FX/npm" sh scripts/release.sh __publish-npm-dry --out "$FX/out" "$1") > "$FX/o" 2>&1
}
# "view NAME" or "publish NAME" per call (a name's parts start with a letter, a version with a digit)
order() { sed -En 's/^npm (view|publish) (.* |.*\/)(sheepr(-[a-z][a-z0-9]*)*)[-@][0-9].*$/\1 \3/p' "$FX/calls" 2>/dev/null | tr '\n' ','; }
D=$FX/out/v0.1.0
mkout "$D" v0.1.0
mkout "$FX/out/v0.1.0-unsigned" v0.1.0
pub v0.1.0; r=$?
[ $r = 0 ] && pass "a dry publish-npm: done" || fail "dry publish-npm: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
[ "$(order)" = "view sheepr-linux-arm64,publish sheepr-linux-arm64,view sheepr-linux-x64,publish sheepr-linux-x64,view sheepr-darwin-universal,publish sheepr-darwin-universal,view sheepr,publish sheepr," ] \
  && pass "the order: each package asked for, then published; platform packages first, the main one last" || fail "the order: $(order)"
# each publish: the registry pinned, no --tag (a final), and a private copy of <out>/v0.1.0's file
# whose bytes, when npm got it, were the manifest's
REG="--registry=https://registry.npmjs.org/"
bad=""; for p in $PK; do
  l=$(grep "^npm publish .*/$p-0.1.0.tgz\$" "$FX/calls"); f=${l##* }
  case $l in "npm publish --access public $REG /private/tmp/sr-npmpub."*"/$p-0.1.0.tgz") ;; *) bad="$bad [$l]" ;; esac
  [ "$(awk -v p="$p" '$1 == p {print $2}' "$FX/pubsha")" = "$(shasum -a 256 "$D/$p-0.1.0.tgz" | cut -d' ' -f1)" ] || bad="$bad [$p: not the manifest's bytes]"
done
[ -z "$bad" ] && pass "each publish: the registry pinned, no --tag (a final), a private copy holding the manifest's bytes" || fail "publish lines:$bad"
[ "$(grep -c "^npm view --json --prefer-online $REG sheepr[a-z0-9-]*@0.1.0 dist.integrity\$" "$FX/calls")" = 4 ] \
  && pass "each view: --json, --prefer-online (not npm's cache), the registry pinned" || fail "view lines: $(grep '^npm view' "$FX/calls" | head -1)"
[ "$(sed -n 1,5p "$FX/calls" | tr '\n' ',')" = "npm whoami $REG,npm owner ls sheepr-linux-arm64 $REG,npm owner ls sheepr-linux-x64 $REG,npm owner ls sheepr-darwin-universal $REG,npm owner ls sheepr $REG," ] \
  && pass "before any view or upload: npm whoami, then the owners of all four names, the registry pinned" || fail "the first calls: $(sed -n 1,5p "$FX/calls" | tr '\n' ',')"
grep -q 'unsigned' "$FX/calls" && fail "a call names the -unsigned sibling" || pass "the -unsigned sibling's packages are never named"
pd=$(awk 'NR == 1 {print $3}' "$FX/pubsha"); pd=${pd%/*}
case $pd in /private/tmp/sr-npmpub.*) [ ! -e "$pd" ] && pass "this run's private copy dir is gone" || fail "$pd is left" ;; *) fail "no private copy dir recorded: '$pd'" ;; esac
# npm's environment class (npm_env, the real run's too): only the named variables; a proxy reaches
# it (the control), a token or an npmrc of the caller's never does in the dry run
bad=""; for f in "$FX"/env.npm.*; do for k in $(sed 's/=.*//' "$f"); do
  case $k in HOME|PATH|TMPDIR|USER|LOGNAME|PWD|SHLVL|_|OLDPWD|HTTPS_PROXY) ;; *) bad="$bad $k" ;; esac
done; done
[ -z "$bad" ] && pass "npm got only the named environment" || fail "extra environment:$(printf '%s\n' $bad | sort -u | tr '\n' ' ')"
grep -q '^HTTPS_PROXY=http://127.0.0.1:9$' "$FX/env.npm.0" && pass "control: the proxy reaches npm" || fail "control: no HTTPS_PROXY for npm"
grep -rl "$DECOY" "$FX"/env.npm.* "$FX/calls" "$FX/o" >/dev/null 2>&1 && fail "a decoy token reached npm or the output" || pass "no decoy token reached npm or the output"
h=$(sed -n 's/^HOME=//p' "$FX/env.npm.0"); case $h in /private/tmp/sr-dryhome.*) pass "the dry npm's HOME is a fresh temp dir" ;; *) fail "the dry npm's HOME is '$h'" ;; esac

# who may publish: npm's own answer, for all four names before any view or upload; the main
# package is asked last, so a refusal there proves no platform package went out first
nopub() { ! grep -q -e '^npm publish' -e '^npm view' "$FX/calls"; }
mkout "$D" v0.1.0; : > "$FX/whoamifail"; pub v0.1.0; r=$?; rm -f "$FX/whoamifail"
[ $r = 1 ] && nopub && grep -q 'npm whoami failed' "$FX/o" && pass "not logged in to npm: refused before any view or upload" || fail "whoami fails: rc=$r $(order) $(tail -1 "$FX/o")"
mkout "$D" v0.1.0; : > "$FX/ownerfail.sheepr"; pub v0.1.0; r=$?; rm -f "$FX/ownerfail.sheepr"
[ $r = 1 ] && nopub && grep -q 'sheepr is not on npm yet' "$FX/o" && pass "a name not on npm yet (the main one, asked last): refused before any upload" || fail "not on npm: rc=$r $(order) $(tail -1 "$FX/o")"
mkout "$D" v0.1.0; echo stranger > "$FX/owner.sheepr-linux-x64"; pub v0.1.0; r=$?; rm -f "$FX/owner.sheepr-linux-x64"
[ $r = 1 ] && nopub && grep -q 'sheepr-linux-x64 on npm belongs to stranger, not to lukasco' "$FX/o" && pass "a name owned by someone else: refused before any upload, the owner named" || fail "other owner: rc=$r $(order) $(tail -1 "$FX/o")"
mkout "$D" v0.1.0; echo lukascox > "$FX/owner.sheepr-darwin-universal"; pub v0.1.0; r=$?; rm -f "$FX/owner.sheepr-darwin-universal"
[ $r = 1 ] && nopub && pass "an owner whose name only starts with the user's: refused" || fail "prefix owner: rc=$r $(order)"
mkout "$D" v0.1.0; : > "$FX/ownerbad.sheepr-linux-arm64"; pub v0.1.0; r=$?; rm -f "$FX/ownerbad.sheepr-linux-arm64"
[ $r = 1 ] && nopub && grep -q 'cannot read the owners of sheepr-linux-arm64' "$FX/o" && pass "owners that cannot be read (not E404): refused before any upload" || fail "owner error: rc=$r $(order) $(tail -1 "$FX/o")"
mkout "$D" v0.1.0; printf 'stranger\nlukasco\n' > "$FX/owner.sheepr-linux-arm64"; pub v0.1.0; r=$?; rm -f "$FX/owner.sheepr-linux-arm64"
[ $r = 0 ] && [ "$(grep -c '^npm publish' "$FX/calls")" = 4 ] && pass "control: the user is one of several owners: published" || fail "co-owner: rc=$r $(order) $(tail -1 "$FX/o")"
# every package is checked before the first npm call: one changed before the run means none is published
mkout "$D" v0.1.0; echo x >> "$D/sheepr-0.1.0.tgz"; pub v0.1.0; r=$?
[ $r = 1 ] && [ ! -e "$FX/calls" ] && pass "a package changed after npm-check, before the run: refused, npm never called" || fail "changed before the run: rc=$r $(order)"
# a view that answers nothing (exit 0, no integrity) cannot say the version is absent
mkout "$D" v0.1.0; : > "$FX/viewempty.sheepr-linux-arm64"; pub v0.1.0; r=$?; rm -f "$FX/viewempty.sheepr-linux-arm64"
[ $r = 1 ] && ! grep -q '^npm publish' "$FX/calls" && grep -q 'cannot tell' "$FX/o" && pass "a view with no answer: refused, nothing published" || fail "empty view: rc=$r $(order)"
# a refused run leaves its private copy dir behind neither
mkout "$D" v0.1.0; : > "$FX/viewfail.sheepr-darwin-universal"; pub v0.1.0; rm -f "$FX/viewfail.sheepr-darwin-universal"
pd=$(awk 'NR == 1 {print $3}' "$FX/pubsha"); pd=${pd%/*}
case $pd in /private/tmp/sr-npmpub.*) [ ! -e "$pd" ] && pass "a refused run's private copy dir is gone" || fail "$pd is left after a refusal" ;; *) fail "no private copy dir recorded after the refusal: '$pd'" ;; esac

# the real run's npm environment (the npm class; __npm-env prints it): the operator's npmrc
# location passes, upper or lower case, and so does a proxy; no token ever does
ne() { (cd "$REPO" && env -i PATH="$PATH" HOME="$FX/ghome" NPM_TOKEN="$DECOY" NODE_AUTH_TOKEN="$DECOY" npm_config__authToken="$DECOY" GH_TOKEN="$DECOY" \
  SSH_AUTH_SOCK="$FX/sock" HTTPS_PROXY=http://127.0.0.1:9 "$@" sh scripts/release.sh __npm-env v0.1.0) 2>&1; }
e1=$(ne NPM_CONFIG_USERCONFIG="$FX/u1/npmrc"); e2=$(ne npm_config_userconfig="$FX/u2/npmrc")
printf '%s\n' "$e1" | grep -qx "NPM_CONFIG_USERCONFIG=$FX/u1/npmrc" && printf '%s\n' "$e2" | grep -qx "NPM_CONFIG_USERCONFIG=$FX/u2/npmrc" \
  && printf '%s\n' "$e1" | grep -qx 'HTTPS_PROXY=http://127.0.0.1:9' && printf '%s\n' "$e1" | grep -qx "HOME=$FX/ghome" \
  && pass "the real npm class: the operator's npmrc (either spelling), HOME and a proxy pass" || fail "the npm class: $(printf '%s' "$e1" | tr '\n' ' ')"
printf '%s\n%s\n' "$e1" "$e2" | grep -q -e "$DECOY" -e '^SSH_AUTH_SOCK=' -e '^GH_' && fail "the npm class let a token or a transport through" || pass "the real npm class: no token, no git/gh transport"

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
mkout "$D" v0.1.0; echo "echo x >> '$D/sheepr-darwin-universal-0.1.0.tgz'" > "$FX/onview.sheepr-darwin-universal"
h0=$(sed -n 's/.*"sheepr-darwin-universal-0.1.0.tgz", "sha256": "\([0-9a-f]*\)".*/\1/p' "$D/MANIFEST.json")
pub v0.1.0; r=$?; rm -f "$FX/onview.sheepr-darwin-universal"
[ $r = 0 ] && [ "$(awk '$1 == "sheepr-darwin-universal" {print $2}' "$FX/pubsha")" = "$h0" ] \
  && pass "a source changed between its check and its upload: npm got the checked bytes" || fail "changed after the check: rc=$r $(cat "$FX/pubsha" 2>/dev/null | tr '\n' ' ')"
# a view that fails for another reason than E404 cannot say the version is absent: refused there
mkout "$D" v0.1.0; : > "$FX/viewfail.sheepr-darwin-universal"; pub v0.1.0; r=$?; rm -f "$FX/viewfail.sheepr-darwin-universal"
[ $r = 1 ] && [ "$(order)" = "view sheepr-linux-arm64,publish sheepr-linux-arm64,view sheepr-linux-x64,publish sheepr-linux-x64,view sheepr-darwin-universal," ] && grep -q 'cannot tell' "$FX/o" \
  && pass "a view that fails (not E404): refused there, nothing after it published" || fail "view failure: rc=$r $(order) $(tail -1 "$FX/o")"

# a package changed during the run (by the first upload, as anything else could): refused at it
mkout "$D" v0.1.0; echo "echo x >> '$D/sheepr-darwin-universal-0.1.0.tgz'" > "$FX/onpublish.sheepr-linux-arm64"
pub v0.1.0; r=$?; rm -f "$FX/onpublish.sheepr-linux-arm64"
[ $r = 1 ] && [ "$(order)" = "view sheepr-linux-arm64,publish sheepr-linux-arm64,view sheepr-linux-x64,publish sheepr-linux-x64," ] \
  && grep -q 'sheepr-darwin-universal-0.1.0.tgz does not match its manifest hash' "$FX/o" \
  && pass "a package changed during the run: refused just before its upload, nothing after it published" || fail "changed mid-run: rc=$r $(order) $(tail -1 "$FX/o")"

# a resumed run: the same file already there is skipped; another file there stops the run
mkout "$D" v0.1.0; integrity "$D/sheepr-linux-arm64-0.1.0.tgz" > "$FX/view.sheepr-linux-arm64"
pub v0.1.0; r=$?
[ $r = 0 ] && ! grep -q '^npm publish .*linux-arm64' "$FX/calls" && [ "$(grep -c '^npm publish' "$FX/calls")" = 3 ] && grep -q 'already on npm (the same file)' "$FX/o" \
  && pass "a version already on npm with this file: skipped, the rest published" || fail "resume same: rc=$r $(order)"
echo "sha512-AAAA" > "$FX/view.sheepr-linux-x64"
pub v0.1.0; r=$?; rm -f "$FX"/view.*
[ $r = 1 ] && [ "$(order)" = "view sheepr-linux-arm64,view sheepr-linux-x64," ] && grep -q 'another file' "$FX/o" \
  && pass "a version already on npm with another file: refused there, nothing after it published" || fail "resume other: rc=$r $(order) $(tail -1 "$FX/o")"

# an rc: every publish carries --tag next
fx_release 0.1.1 2 v0.1.1-rc.1
mkout "$FX/out/v0.1.1-rc.1" v0.1.1-rc.1
pub v0.1.1-rc.1; r=$?
[ $r = 0 ] && [ "$(grep -c "^npm publish --access public $REG --tag next " "$FX/calls")" = 4 ] && pass "an rc: each publish has --tag next" || fail "rc: rc=$r $(grep '^npm publish' "$FX/calls" | head -1)"

# __publish-npm-dry refuses a stand-in outside the fixtures; the outside one is never called
OUT=$(mktemp -d /private/tmp/sr-outside.XXXXXX); trap 'rm -rf "$OUT" "$FX"' EXIT
printf '#!/bin/sh\necho "OUTSIDE $*" >> "%s/called"\nexit 0\n' "$OUT" > "$OUT/npm"; chmod +x "$OUT/npm"
ln -s "$OUT/npm" "$FX/npm-link"
for spec in "outside:$OUT/npm" "symlink:$FX/npm-link"; do
  l=${spec%%:*} p=${spec#*:}; rm -f "$OUT/called"
  (cd "$REPO" && env HOME="$FX/ghome" SR_PUBLISH_DRY_NPM="$p" sh scripts/release.sh __publish-npm-dry --out "$FX/out" v0.1.0) > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && [ ! -e "$OUT/called" ] && pass "__publish-npm-dry, a $l npm: refused, the outside npm never called" || fail "__publish-npm-dry, $l: rc=$r"
done
n=$(ls -d /private/tmp/sr-dryhome.* 2>/dev/null | wc -l | tr -d ' ')
[ "$n" = 0 ] && pass "no dry HOME is left, after the refused runs too" || fail "$n dry HOME dirs left"
finish
