#!/bin/sh
# PHASE3.md §1.2: release.sh's publish executor, dry (`__publish-dry`: gh and git are stand-ins
# given by path under /private/tmp/sr-p3-fixtures.*, never the real ones; verify is not run; the
# confirmations come from a script). In the operator's locale (it sorts SHA256SUMS after install.sh;
# release.sh sets LC_ALL=C) and with decoy tokens in the environment:
#   - the whole run: the remote tag read from github.com/lukaso/sheepr itself, the releases
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
#   - __publish-dry refuses stand-ins outside /private/tmp/sr-p3-fixtures.*;
#   - the Homebrew cask (a final tag): before anything is public, the out dir's cask is the one
#     the tag's own template renders, the tap's cask is read (its version older, or the same bytes)
#     and the tap can be written; after the PATCH, the release read by its id, its archive
#     downloaded and fetched as brew fetches it (curl, no credentials), then one PUT carrying the
#     blob id it replaces, and the file read back at the PUT's commit. A failure after the PATCH
#     exits 5 and names `publish-cask`; `__publish-cask-dry` (that step alone) resumes it. The
#     render must hold exactly one version (the tag's), sha256 (the archive's manifest hash) and
#     url (the release's archive) line; after its checks the cask step reads nothing of the out
#     dir (its cask or its manifest changed at the PATCH never reaches the tap); a backport
#     publishes with the cask untouched; curl's real class (__curl-env, from real_tools), the real
#     entries' and real_tools' bodies (each defined once) and their dispatch arms are pinned.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
C=$(g rev-parse "v0.1.0^{commit}"); T=$(g rev-parse v0.1.0)
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
mkout() { # dir tag commit: the five files, the manifest naming each one's hash, and npm-check's
           # stamp for that manifest (as a passed npm-check leaves it)
  rm -rf "$1"; mkdir -p "$1"; v=${2#v}
  for f in sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh \
           sheepr-$v.tgz sheepr-darwin-universal-$v.tgz sheepr-linux-arm64-$v.tgz sheepr-linux-x64-$v.tgz; do echo "$f $1" > "$1/$f"; done
  (cd "$1" && shasum -a 256 sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh > SHA256SUMS)
  { printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "signed",\n  "control": false,\n  "files": [\n' "$2" "$3"
    sep=""; for f in sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh \
                     sheepr-$v.tgz sheepr-darwin-universal-$v.tgz sheepr-linux-arm64-$v.tgz sheepr-linux-x64-$v.tgz; do
      printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$f" "$(shasum -a 256 "$1/$f" | cut -d' ' -f1)"; sep=",
"; done; printf '\n  ]\n}\n'; } > "$1/MANIFEST.json"
  printf '%s %s\n' "$2" "$(shasum -a 256 "$1/MANIFEST.json" | cut -d' ' -f1)" > "$1/NPM-CHECKED"
  # the cask, as `build` renders it (render-cask.sh, the archive's hash)
  sh "$REPO/scripts/lib/render-cask.sh" "$v" "$(shasum -a 256 "$1/sheepr-macos-universal.tar.gz" | cut -d' ' -f1)" "$1/sheepr.rb" || exit 3
}
# the tap's cask, as the contents API serves it: a cask rendered for VERSION (HASH: another archive)
tapset() { rm -rf "$FX/tap"; mkdir -p "$FX/tap"; sh "$REPO/scripts/lib/render-cask.sh" "$1" "${2:-$(printf '%064d' 7)}" "$FX/tap/sheepr.rb" || exit 3; }
blob() { { printf 'blob %d\0' "$(wc -c < "$1" | tr -d ' ')"; cat "$1"; } | shasum -a 1 | cut -d' ' -f1; }
# the stand-ins
cat > "$FX/gh" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.gh\\.'); env > "$FX/env.gh.\$n"
echo "gh \$*" >> "$FX/calls"
case "\$*" in
  *"repos/lukaso/homebrew-tap --jq .permissions.push"*) cat "$FX/tapperm" 2>/dev/null || echo true ;;
  *"-X PUT repos/lukaso/homebrew-tap/contents/Casks/sheepr.rb"*)
    [ -e "$FX/put409" ] && { echo "gh: Conflict (HTTP 409)" >&2; exit 1; }
    for a; do case \$a in content=*) printf '%s' "\${a#content=}" | base64 -d > "$FX/tap/sheepr.rb" ;; message=*) printf '%s\n' "\${a#message=}" > "$FX/tap/message" ;; sha=*) printf '%s\n' "\${a#sha=}" > "$FX/tap/putsha" ;; esac; done
    echo c0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ff ;;
  *"homebrew-tap/contents/Casks/sheepr.rb?ref=c0ffee"*) cat "$FX/tap/sheepr.rb"; [ -e "$FX/readbackdiff" ] && echo x; exit 0 ;;
  *"homebrew-tap/contents/Casks/sheepr.rb"*) [ -f "$FX/tap/sheepr.rb" ] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }; cat "$FX/tap/sheepr.rb" ;;
  *"releases/tags/"*"--jq .draft, (.assets"*|*"releases/4242 --jq .draft, (.assets"*)
    case "\$*" in *releases/tags/*) [ -e "$FX/notpublished" ] && { echo "gh: Not Found (HTTP 404)" >&2; exit 1; } ;; esac
    cat "$FX/draftafter" 2>/dev/null || echo false
    i=0; for f in \$(ls "$FX/up"); do i=\$((i+1)); [ "\$f" = sheepr-macos-universal.tar.gz ] && echo \$i; done ;;
  *"releases --paginate"*) cat "$FX/existing" 2>/dev/null ;;
  "api -X POST repos/lukaso/sheepr/releases "*) echo 4242 ;;
  *"uploads.github.com"*) prev=""; for a; do case \$a in *assets\\?name=*) nm=\${a##*name=} ;; esac; [ "\$prev" = --input ] && in=\$a; prev=\$a; done
    [ -e "$FX/mutate" ] && [ "\$nm" = "\$(cat "$FX/mutate")" ] && echo changed >> "\$in"
    mkdir -p "$FX/up"; cp "\$in" "$FX/up/\$nm"; echo 1 ;;
  "api repos/lukaso/sheepr/releases/4242 --jq .assets"*) i=0; for f in \$(ls "$FX/up"); do i=\$((i+1)); echo "\$i \$f"; done ;;
  *"releases/assets/"*) for a; do last=\$a; done; id=\${last##*/}; f=\$(ls "$FX/up" | sed -n "\${id}p"); cat "$FX/up/\$f"; [ -e "$FX/corrupt" ] && [ "\$f" = "\$(cat "$FX/corrupt")" ] && echo x
    [ -e "$FX/corruptafter" ] && grep -q PATCH "$FX/calls" && echo x ;;
  *"PATCH"*) [ -e "$FX/swapcask" ] && sed -i.bak 's/^  sha256 .*/  sha256 "1111111111111111111111111111111111111111111111111111111111111111"/' "$FX/out/v0.1.0/sheepr.rb" && rm -f "$FX/out/v0.1.0/sheepr.rb.bak"
    [ -e "$FX/rmcask" ] && rm -f "$FX/out/v0.1.0/sheepr.rb"
    [ -e "$FX/swaphash" ] && sed -i.bak "s/\"sheepr-macos-universal.tar.gz\", \"sha256\": \"[0-9a-f]*\"/\"sheepr-macos-universal.tar.gz\", \"sha256\": \"\$(cat "$FX/swaphash")\"/" "$FX/out/v0.1.0/MANIFEST.json" && rm -f "$FX/out/v0.1.0/MANIFEST.json.bak"
    [ -e "$FX/patchfail" ] && exit 1 ;;
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
cat > "$FX/npm" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.npm\\.'); env > "$FX/env.npm.\$n"
echo "npm \$*" >> "$FX/calls"
case \$1 in owner) p=\$3
  if [ -f "$FX/ownerfail.\$p" ]; then echo "npm error code E404" >&2; exit 1; fi
  if [ -f "$FX/owner.\$p" ]; then echo "\$(cat "$FX/owner.\$p") <x@example.com>"; else echo "lukasco <x@example.com>"; fi; exit 0 ;;
esac
exit 1
EOF
cat > "$FX/curl" <<EOF
#!/bin/sh
n=\$(ls "$FX" | grep -c '^env\\.curl\\.'); env > "$FX/env.curl.\$n"
echo "curl \$*" >> "$FX/calls"
cat "$FX/up/sheepr-macos-universal.tar.gz" 2>/dev/null; [ -e "$FX/curlcorrupt" ] && echo x
exit 0
EOF
chmod +x "$FX/gh" "$FX/git" "$FX/npm" "$FX/curl"
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"
pub() { # tag answers -> rc; output in $FX/o (a fresh tap: TAPV's cask, 0.0.9 when unset; TAPKEEP=1:
       # the tap as it is). Set them as plain assignments and clear them after: a VAR=x prefix on
       # a function call persists in POSIX sh
  rm -rf "$FX/calls" "$FX"/env.* "$FX/up"; printf '%s\n' "$2" > "$FX/ask"
  [ "${TAPKEEP:-}" = 1 ] || tapset "${TAPV:-0.0.9}"
  (cd "$REPO" && env -u LC_ALL LANG=en_GB.UTF-8 LC_COLLATE=en_GB.UTF-8 HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    GH_TOKEN="$DECOY" GITHUB_TOKEN="$DECOY" APPLE_APP_SPECIFIC_PASSWORD="$DECOY" NPM_TOKEN="$DECOY" GH_CONFIG_DIR="$FX/ghcfg" SSH_AUTH_SOCK="$FX/sock" \
    SR_PUBLISH_DRY_GH="$FX/gh" SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$FX/npm" SR_PUBLISH_DRY_CURL="$FX/curl" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls" \
    sh scripts/release.sh __publish-dry --out "$FX/out" "$1") > "$FX/o" 2>&1
}
cask() { # tag -> rc: `__publish-cask-dry` (the cask step alone); the tap as it is; output in $FX/o
  rm -rf "$FX/calls" "$FX"/env.*
  (cd "$REPO" && env -u LC_ALL HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GH_TOKEN="$DECOY" \
    SR_PUBLISH_DRY_GH="$FX/gh" SR_PUBLISH_DRY_CURL="$FX/curl" sh scripts/release.sh __publish-cask-dry --out "$FX/out" "$1") > "$FX/o" 2>&1
}
nowrite() { ! grep -q -e '^gh api -X PUT' "$FX/calls"; }
nopublic() { ! grep -q -e '^gh api -X POST' -e '^gh api -X PATCH' "$FX/calls"; }
seq() { sed -e '/^npm /d' -e 's/^gh api -H .*homebrew-tap.*ref=.*/READBACK/; s/^gh api -H .*homebrew-tap.*/TAPGET/; s/^gh api repos\/lukaso\/homebrew-tap --jq.*/TAPPERM/; s/^gh api -X PUT.*/PUT/; s/^curl .*/CURL/' \
  -e 's/^gh api -X POST repos.*/POST/; s/^gh api -X POST -H .*/UPLOAD/; s/^gh api repos\/lukaso\/sheepr\/releases --paginate.*/LIST/' \
  -e 's/^gh api repos\/lukaso\/sheepr\/releases\/4242 .*/READ/; s/^gh api -H .*assets.*/DL/; s/^gh api -X PATCH.*/PATCH/' \
  -e 's/^git ls-remote .*/LSREMOTE/; s/^ask .*/ASK/' "$FX/calls" | tr '\n' ' '; }

mkout "$FX/out/v0.1.0" v0.1.0 "$C"
pub v0.1.0 v0.1.0; r=$?
[ $r = 0 ] && pass "a dry publish in the operator's locale: done" || fail "dry publish: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
[ "$(seq)" = "TAPGET TAPPERM LSREMOTE LIST POST UPLOAD UPLOAD UPLOAD UPLOAD UPLOAD READ DL DL DL DL DL LSREMOTE ASK PATCH READ DL CURL PUT READBACK " ] \
  && pass "the order: the tap read and its write right checked, tag, list, draft, 5 uploads, read, 5 downloads, tag again, confirm, publish; then the release read, its archive downloaded and fetched as brew does, the cask PUT and read back" || fail "the order: $(seq)"
cmp -s "$FX/tap/sheepr.rb" "$FX/out/v0.1.0/sheepr.rb" && pass "the tap now holds the out dir's cask, byte for byte" || fail "the tap's cask is not the out dir's"
[ "$(cat "$FX/tap/message")" = "sheepr 0.1.0" ] && pass "the cask commit's message is 'sheepr 0.1.0'" || fail "the message: $(cat "$FX/tap/message" 2>/dev/null)"
tapset 0.0.9; want=$(blob "$FX/tap/sheepr.rb"); pub v0.1.0 v0.1.0
[ "$(cat "$FX/tap/putsha" 2>/dev/null)" = "$want" ] && pass "the PUT carries the blob id of the cask it read (GitHub refuses it if the file changed since)" || fail "the PUT's sha: $(cat "$FX/tap/putsha" 2>/dev/null), want $want"
grep -q '^curl -q -fsSL --proto =https https://github.com/lukaso/sheepr/releases/download/v0.1.0/sheepr-macos-universal.tar.gz$' "$FX/calls" \
  && pass "the archive is fetched at the cask's url, with no curlrc and https only" || fail "the curl call: $(grep '^curl' "$FX/calls")"
# the npm names are checked before anything else: GitHub must not go public when npm would then
# refuse (the first v0.1.0 did exactly that). `npm owner ls` needs no login; the owner is release.conf's
REG="--registry=https://registry.npmjs.org/"
[ "$(sed -n 1,4p "$FX/calls" | tr '\n' ',')" = "npm owner ls sheepr-linux-arm64 $REG,npm owner ls sheepr-linux-x64 $REG,npm owner ls sheepr-darwin-universal $REG,npm owner ls sheepr $REG," ] \
  && pass "before any git or gh call: the npm owners of all four names" || fail "the first calls: $(sed -n 1,4p "$FX/calls" | tr '\n' ',')"
[ "$(grep -c '^git ls-remote https://github.com/lukaso/sheepr refs/tags/v0.1.0\*$' "$FX/calls")" = 2 ] \
  && pass "the tag is read from github.com/lukaso/sheepr itself (twice)" || fail "ls-remote: $(grep ls-remote "$FX/calls" | head -1)"
bad=""; for f in "$FX"/env.gh.* "$FX"/env.git.*; do for k in $(sed 's/=.*//' "$f"); do   # npm has its own class and row
  case $k in HOME|PATH|TMPDIR|USER|LOGNAME|PWD|SHLVL|_|OLDPWD) ;; *) bad="$bad $k" ;; esac
done; done
[ -z "$bad" ] && pass "gh and git got only the named environment" || fail "extra environment:$(printf '%s\n' $bad | sort -u | tr '\n' ' ')"
grep -rl "$DECOY" "$FX"/env.* "$FX/calls" "$FX/o" >/dev/null 2>&1 && fail "a decoy token reached gh, git or the output" || pass "no decoy token reached gh, git or the output"

echo sheepr-linux-x86_64 > "$FX/corrupt"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/corrupt"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "a download that differs: no PATCH (1)" || fail "corrupt download: rc=$r"
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" 0000000000000000000000000000000000000000 > "$FX/moved"
pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/moved"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "the tag moved during the upload: no PATCH (1)" || fail "moved tag: rc=$r"
pub v0.1.0 no; r=$?
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && pass "a wrong second answer: no PATCH (1)" || fail "wrong answer: rc=$r"

# a file changed during its own upload (the upload and the local file agree, the manifest does
# not): the downloads are compared with the manifest, so no PATCH
echo sheepr-linux-aarch64 > "$FX/mutate"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/mutate"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && grep -q 'manifest' "$FX/o" && pass "a file changed during its upload: no PATCH, the manifest named (1)" || fail "changed during upload: rc=$r $(tail -1 "$FX/o")"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# SHA256SUMS (not in the manifest): its hash is the one the planner checked, before any upload
echo SHA256SUMS > "$FX/mutate"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/mutate"
[ $r = 1 ] && ! grep -q PATCH "$FX/calls" && grep -q 'SHA256SUMS is not the one the build wrote' "$FX/o" && pass "SHA256SUMS changed during its upload: no PATCH, said so (1)" || fail "SHA256SUMS changed during upload: rc=$r $(tail -1 "$FX/o")"
# SHA256SUMS must be the build's bytes (from the manifest's four hashes, in build order): one that
# still checks out but is not (its lines reordered) is refused before any call
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; (cd "$FX/out/v0.1.0" && sort -r -k2 SHA256SUMS > S && mv S SHA256SUMS && shasum -a 256 -c --strict SHA256SUMS >/dev/null) || fail "could not reorder SHA256SUMS"
pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q 'SHA256SUMS is not the one the build wrote' "$FX/o" && pass "a SHA256SUMS that is not the build's (reordered): refused before any call" || fail "reordered SHA256SUMS: rc=$r calls=$(seq)"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# every file npm-check checked is still the manifest's: an npm package changed or gone after it
# is refused before any call
for x in change rm; do
  mkout "$FX/out/v0.1.0" v0.1.0 "$C"
  if [ $x = change ]; then echo x >> "$FX/out/v0.1.0/sheepr-darwin-universal-0.1.0.tgz"; else rm "$FX/out/v0.1.0/sheepr-0.1.0.tgz"; fi
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
[ $r = 0 ] && grep -q 'v0.1.0 is public' "$FX/o" && grep -q '^gh api -X PUT' "$FX/calls" && cmp -s "$FX/tap/sheepr.rb" "$FX/out/v0.1.0/sheepr.rb" \
  && pass "the PATCH failed but the release is public: said so, and the cask step still ran (0)" || fail "patch failed, public: rc=$r $(seq) $(tail -1 "$FX/o")"
echo true > "$FX/draftstate"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && nowrite && grep -q 'still a draft' "$FX/o" && grep -q 'draft 4242' "$FX/o" && grep -q 'delete' "$FX/o" && pass "the PATCH failed and the release is still a draft: said so, with the draft and the way on (1)" || fail "patch failed, draft: rc=$r $(tail -1 "$FX/o")"
echo fail > "$FX/draftstate"; pub v0.1.0 v0.1.0; r=$?
[ $r = 1 ] && grep -q 'check it on GitHub' "$FX/o" && pass "the PATCH failed and the state cannot be read: said so (1)" || fail "patch failed, unreadable: rc=$r $(tail -1 "$FX/o")"
rm -f "$FX/patchfail" "$FX/draftstate"

fx_release 0.1.1 2 v0.1.1-rc.1; C2=$(g rev-parse "v0.1.1-rc.1^{commit}")
printf '%s\trefs/tags/v0.1.1-rc.1\n' "$C2" > "$FX/remote"
mkout "$FX/out/v0.1.1-rc.1" v0.1.1-rc.1 "$C2"
pub v0.1.1-rc.1 v0.1.1-rc.1; r=$?
[ $r = 0 ] && grep -q '^gh api -X POST repos/lukaso/sheepr/releases -F draft=true -F prerelease=true ' "$FX/calls" && pass "an rc tag: the draft is a prerelease" || fail "rc: rc=$r $(grep POST "$FX/calls" | head -1)"
! grep -q -e homebrew-tap -e '^curl' "$FX/calls" && grep -q 'an rc: the Homebrew cask is not touched' "$FX/o" && pass "an rc tag: no call to the tap or curl, said so" || fail "rc and the tap: $(grep -e homebrew-tap -e '^curl' "$FX/calls" | head -1) $(tail -1 "$FX/o")"
cask v0.1.1-rc.1; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q 'an rc' "$FX/o" && pass "publish-cask of an rc: refused before any call (1)" || fail "publish-cask rc: rc=$r $(seq) $(tail -1 "$FX/o")"

# the cask, before anything is public: each refusal comes before the POST (nothing public), names
# what it saw, and writes nothing to the tap
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"
pre() { # label want-text: the last pub refused before anything public
  [ $r = 1 ] && nopublic && nowrite && grep -q "$2" "$FX/o" && pass "$1: refused before anything is public (1)" || fail "$1: rc=$r $(seq) $(tail -1 "$FX/o")"
}
echo '# edited' >> "$FX/out/v0.1.0/sheepr.rb"; pub v0.1.0 v0.1.0; r=$?; mkout "$FX/out/v0.1.0" v0.1.0 "$C"
pre "an out-dir cask that is not the one the tag renders" "is not the cask the tag renders"
TAPV=0.2.0; pub v0.1.0 v0.1.0; r=$?; TAPV=
[ $r = 0 ] && grep -q PATCH "$FX/calls" && nowrite && ! grep -q '^curl' "$FX/calls" && grep -q "cask is 0.2.0, newer than v0.1.0: the Homebrew cask is not touched" "$FX/o" \
  && pass "an older final tag than the tap's cask (a backport): published, the cask not touched, said so (0)" || fail "backport: rc=$r $(seq) $(tail -1 "$FX/o")"
tapset 0.2.0; cask v0.1.0; r=$?
[ $r = 1 ] && nowrite && grep -q 'newer than v0.1.0' "$FX/o" && pass "publish-cask of an older tag than the tap's cask: refused, nothing written (1)" || fail "publish-cask backport: rc=$r $(seq) $(tail -1 "$FX/o")"
TAPV=0.1.0-rc.9; pub v0.1.0 v0.1.0; r=$?; TAPV=
[ $r = 0 ] && grep -q '^gh api -X PUT' "$FX/calls" && pass "control: the tap's cask is an rc of this version (0.1.0-rc.9): updated (0)" || fail "tap rc of this version: rc=$r $(tail -1 "$FX/o")"
tapset 0.1.0; TAPKEEP=1; pub v0.1.0 v0.1.0; r=$?; TAPKEEP=; pre "the tap's cask is this version with another archive hash" "0000000000000000000000000000000000000000000000000000000000000007"
grep -q "$(shasum -a 256 "$FX/out/v0.1.0/sheepr-macos-universal.tar.gz" | cut -d' ' -f1)" "$FX/o" && pass "  that refusal names both hashes" || fail "  the refusal names: $(tail -1 "$FX/o")"
tapset 0.0.9; sed 's/^\(  version .*\)$/\1\n\1/' "$FX/tap/sheepr.rb" > "$FX/t2" && mv "$FX/t2" "$FX/tap/sheepr.rb"
TAPKEEP=1; pub v0.1.0 v0.1.0; r=$?; TAPKEEP=; pre "the tap's cask with two version lines" "one version line"
rm -rf "$FX/tap"; mkdir -p "$FX/tap"; TAPKEEP=1; pub v0.1.0 v0.1.0; r=$?; TAPKEEP=; pre "no Casks/sheepr.rb in the tap" "has no Casks/sheepr.rb"
echo false > "$FX/tapperm"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/tapperm"; pre "no right to write the tap" "cannot write lukaso/homebrew-tap"
# the tap already holds this cask: no PUT (a resumed run), the file read again
cp "$FX/out/v0.1.0/sheepr.rb" "$FX/tap.cur"; tapset 0.0.9; cp "$FX/tap.cur" "$FX/tap/sheepr.rb"; TAPKEEP=1; pub v0.1.0 v0.1.0; r=$?; TAPKEEP=
[ $r = 0 ] && nowrite && grep -q 'already' "$FX/o" && pass "the tap already holds this cask: no PUT (0)" || fail "tap current: rc=$r $(seq) $(tail -1 "$FX/o")"

# after the PATCH, a failure leaves the release public: exit 5, the next command named
post() { # label: the last pub ended at 5, public, publish-cask named
  [ $r = 5 ] && grep -q PATCH "$FX/calls" && grep -q 'v0.1.0 is public' "$FX/o" && grep -qF "publish-cask --out \"$FX/out\" v0.1.0" "$FX/o" \
    && pass "$1: exit 5, public said, publish-cask named" || fail "$1: rc=$r $(seq) $(tail -2 "$FX/o" | tr '\n' ' ')"
}
touch "$FX/corruptafter"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/corruptafter"; post "the published archive is not the build's"; nowrite && pass "  and no PUT" || fail "  a PUT ran"
touch "$FX/curlcorrupt"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/curlcorrupt"; post "the archive brew would fetch is not the build's"; nowrite && pass "  and no PUT" || fail "  a PUT ran"
echo true > "$FX/draftafter"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/draftafter"; post "the release reads as a draft after the PATCH"; nowrite && pass "  and no PUT" || fail "  a PUT ran"
touch "$FX/put409"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/put409"; post "the PUT is refused (the tap changed)"
touch "$FX/readbackdiff"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/readbackdiff"; post "the cask read back is not the one put"
# publish-cask alone resumes it: the release is public, the tap is updated
touch "$FX/put409"; pub v0.1.0 v0.1.0 >/dev/null; rm -f "$FX/put409"; cask v0.1.0; r=$?
[ $r = 0 ] && grep -q '^gh api -X PUT' "$FX/calls" && cmp -s "$FX/tap/sheepr.rb" "$FX/out/v0.1.0/sheepr.rb" && ! grep -q -e 'POST' -e 'PATCH' "$FX/calls" \
  && pass "publish-cask after a failed cask step: the cask put, nothing else published (0)" || fail "publish-cask resume: rc=$r $(seq) $(tail -1 "$FX/o")"
cask v0.1.0; r=$?
[ $r = 0 ] && nowrite && pass "publish-cask again: the tap is current, no PUT (0)" || fail "publish-cask current: rc=$r $(seq) $(tail -1 "$FX/o")"
touch "$FX/notpublished"; tapset 0.0.9; cask v0.1.0; r=$?; rm -f "$FX/notpublished"
[ $r = 1 ] && nowrite && grep -q 'no published release' "$FX/o" && ! grep -q 'is public' "$FX/o" && pass "publish-cask with no published release: refused, nothing written, 'public' never said (1)" || fail "publish-cask unpublished: rc=$r $(seq) $(tail -1 "$FX/o")"
# the cask is rendered from the tag's template, not the checkout's
echo '# a later change' >> "$REPO/packaging/homebrew/sheepr.rb.in"; tapset 0.0.9; cask v0.1.0; r=$?; g checkout -q packaging/homebrew/sheepr.rb.in
[ $r = 0 ] && cmp -s "$FX/tap/sheepr.rb" "$FX/out/v0.1.0/sheepr.rb" && pass "publish-cask with the checkout's template changed after the tag: the tag's renders, the cask put (0)" || fail "template changed: rc=$r $(tail -1 "$FX/o")"
sed -i.bak 's/"commit": "[0-9a-f]*"/"commit": "0000000000000000000000000000000000000000"/' "$FX/out/v0.1.0/MANIFEST.json" && rm -f "$FX/out/v0.1.0/MANIFEST.json.bak"
tapset 0.0.9; cask v0.1.0; r=$?; mkout "$FX/out/v0.1.0" v0.1.0 "$C"
[ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q "is not v0.1.0's" "$FX/o" && pass "publish-cask with a manifest of another commit: refused before any call (1)" || fail "publish-cask commit: rc=$r $(seq) $(tail -1 "$FX/o")"
# the cask put is the one checked before anything was public: a change to the out dir while the
# operator confirms (at the PATCH) never reaches the tap
cp "$FX/out/v0.1.0/sheepr.rb" "$FX/checked.rb"
for x in swapcask rmcask; do
  mkout "$FX/out/v0.1.0" v0.1.0 "$C"; touch "$FX/$x"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/$x"
  [ $r = 0 ] && cmp -s "$FX/tap/sheepr.rb" "$FX/checked.rb" && pass "the out dir's cask $( [ $x = swapcask ] && echo changed || echo removed) at the PATCH: the tap gets the bytes checked before (0)" || fail "$x: rc=$r $(seq) $(tail -1 "$FX/o")"
done
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# nothing of the out dir is read after the checks: the manifest's archive hash changed at the PATCH
# (with the published archive changed to match it) is a mismatch (5, no PUT); the manifest alone
# changed is not read at all (0, the checked cask put)
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; cp "$FX/out/v0.1.0/sheepr.rb" "$FX/checked.rb"
{ cat "$FX/out/v0.1.0/sheepr-macos-universal.tar.gz"; echo x; } | shasum -a 256 | cut -d' ' -f1 > "$FX/swaphash"
touch "$FX/corruptafter" "$FX/curlcorrupt"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/corruptafter" "$FX/curlcorrupt"
post "the manifest and the published archive changed together at the PATCH"; nowrite && pass "  and no PUT" || fail "  a PUT ran"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"; printf '%064d\n' 3 > "$FX/swaphash"; pub v0.1.0 v0.1.0; r=$?
[ $r = 0 ] && cmp -s "$FX/tap/sheepr.rb" "$FX/checked.rb" && pass "the manifest alone changed at the PATCH: not read again, the checked cask put (0)" || fail "manifest changed: rc=$r $(seq) $(tail -1 "$FX/o")"
rm -f "$FX/swaphash"; mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# a tag whose template hard-codes the version or the sha256 renders the same in the build, so the
# out dir matches: the render's version, sha256 and url lines are each checked (one, and the
# release's), before anything is public
n=6
for edit in 's|^  version "@VERSION@"|  version "9.9.9"|' 's|^  sha256 "@SHA256@"|  sha256 "1111111111111111111111111111111111111111111111111111111111111111"|' 's|^  sha256 "@SHA256@"|  sha256 "@SHA256@"\n  sha256 "@SHA256@"|'; do
  sed -i.bak "$edit" "$REPO/packaging/homebrew/sheepr.rb.in" && rm -f "$REPO/packaging/homebrew/sheepr.rb.in.bak"
  g commit -qam "a template edit $n"; fx_release 0.1.$n 4 v0.1.$n; Cn=$(g rev-parse "v0.1.$n^{commit}"); Tn=$(g rev-parse v0.1.$n)
  mkout "$FX/out/v0.1.$n" v0.1.$n "$Cn"; printf '%s\trefs/tags/v0.1.%s\n%s\trefs/tags/v0.1.%s^{}\n' "$Tn" "$n" "$Cn" "$n" > "$FX/remote"
  pub v0.1.$n v0.1.$n; r=$?; pre "a tag whose template has $edit" "cask is not this release's"
  cask v0.1.$n; r=$?
  [ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q "cask is not this release's" "$FX/o" && pass "  publish-cask of it: refused before any call (1)" || fail "  publish-cask: rc=$r $(seq) $(tail -1 "$FX/o")"
  g checkout -q v0.1.0 -- packaging/homebrew/sheepr.rb.in; g commit -qam "the template back" >/dev/null; n=$((n + 1))
done
printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"; mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# the real entries' wiring, which no dry run reaches: both verify the archive first, and both take
# their tools from one definition, which __curl-env (above) prints the curl class of
body() { sed -n "/^$1() {/,/^}/p" "$SR_ROOT/scripts/release.sh"; }
# each body exactly (a looser match passed `verify ... &`, whose refusal ends only a subshell, and
# real_tools after the run)
want_publish='publish() {
  verify "$out/$tag"
  real_tools
  publish_exec "$out/$tag"
}'
want_publish_cask='publish_cask() {
  verify "$out/$tag"   # the archive the cask points at, by the real tools, as publish checks it
  real_tools
  publish_cask_exec "$out/$tag"
}'
want_real_tools='real_tools() {
  GH=gh GITCMD=git NPM=npm NPMC=npm DRY=no NETC=net CURL=/usr/bin/curl CURLC=web
}'
for e in publish publish_cask real_tools; do
  eval "want=\$want_$e"
  [ "$(body $e)" = "$want" ] && pass "$e: exactly its pinned body" || fail "$e's body: $(body $e | tr '\n' ' ')"
  # defined once, in any spelling (sh runs the last definition)
  n=$(grep -Ec "(^|[;&|[:space:]])$e[[:space:]]*[(][)]" "$SR_ROOT/scripts/release.sh")
  [ "$n" = 1 ] && pass "$e: defined once" || fail "$e is defined $n times"
done
# the dispatch runs the entries themselves
for arm in '  publish) publish ;;' '  publish-cask) publish_cask ;;'; do
  [ "$(grep -c "^  $(printf '%s' "$arm" | sed 's/^  //; s/).*//')) " "$SR_ROOT/scripts/release.sh")" = 1 ] && grep -qxF "$arm" "$SR_ROOT/scripts/release.sh" \
    && pass "the dispatch: '$arm'" || fail "the dispatch arm for '$arm': $(grep -n "^  $(printf '%s' "$arm" | sed 's/^  //; s/).*//'))" "$SR_ROOT/scripts/release.sh")"
done

# a tag whose template gives another url form: refused before anything is public (the url is
# checked in the cask that was rendered), and publish-cask before any call
sed -i.bak 's|releases/download/v#{version}/|releases/download/#{version}/|' "$REPO/packaging/homebrew/sheepr.rb.in" && rm -f "$REPO/packaging/homebrew/sheepr.rb.in.bak"
g commit -qam "another url form"; fx_release 0.1.5 3 v0.1.5; C5=$(g rev-parse "v0.1.5^{commit}"); T5=$(g rev-parse v0.1.5)
mkout "$FX/out/v0.1.5" v0.1.5 "$C5"; printf '%s\trefs/tags/v0.1.5\n%s\trefs/tags/v0.1.5^{}\n' "$T5" "$C5" > "$FX/remote"
pub v0.1.5 v0.1.5; r=$?; pre "a tag whose cask url is not the release's archive" "cask is not this release's: its url line"
cask v0.1.5; r=$?
[ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q "cask is not this release's: its url line" "$FX/o" && pass "publish-cask of that tag: refused before any call (1)" || fail "publish-cask url: rc=$r $(seq) $(tail -1 "$FX/o")"
g checkout -q v0.1.0 -- packaging/homebrew/sheepr.rb.in; printf '%s\trefs/tags/v0.1.0\n%s\trefs/tags/v0.1.0^{}\n' "$T" "$C" > "$FX/remote"
mkout "$FX/out/v0.1.0" v0.1.0 "$C"
# the real run's curl environment (the web class; __curl-env prints it): HOME and a proxy pass, no
# token and no git/gh transport
# (__curl-env takes the class from real_tools, the entries' own definition)
ce=$( (cd "$REPO" && env -i PATH="$PATH" HOME="$FX/ghome" GH_TOKEN="$DECOY" GITHUB_TOKEN="$DECOY" SSH_AUTH_SOCK="$FX/sock" GH_CONFIG_DIR="$FX/ghcfg" \
  CURL_HOME="$FX/ch" HTTPS_PROXY=http://127.0.0.1:9 no_proxy=localhost sh scripts/release.sh __curl-env v0.1.0) 2>&1)
printf '%s\n' "$ce" | grep -qx 'HTTPS_PROXY=http://127.0.0.1:9' && printf '%s\n' "$ce" | grep -qx 'no_proxy=localhost' && printf '%s\n' "$ce" | grep -qx "HOME=$FX/ghome" \
  && pass "the real curl class: HOME and the proxies pass" || fail "the curl class: $(printf '%s' "$ce" | tr '\n' ' ')"
printf '%s\n' "$ce" | grep -q -e "$DECOY" -e '^SSH_AUTH_SOCK=' -e '^GH_' -e '^CURL_HOME=' && fail "the curl class let a token, a transport or CURL_HOME through" || pass "the real curl class: no token, no git/gh transport, no CURL_HOME"

# curl, as gh and git, gets only the named environment, and no decoy
pub v0.1.0 v0.1.0 >/dev/null
bad=""; for f in "$FX"/env.curl.*; do for k in $(sed 's/=.*//' "$f"); do case $k in HOME|PATH|TMPDIR|USER|LOGNAME|PWD|SHLVL|_|OLDPWD) ;; *) bad="$bad $k" ;; esac; done; done
[ -e "$FX/env.curl.0" ] && [ -z "$bad" ] && ! grep -l "$DECOY" "$FX"/env.curl.* >/dev/null 2>&1 && pass "curl got only the named environment, no decoy" || fail "curl's environment:$bad"

# __publish-dry refuses every way to reach a gh outside the fixtures. The "real gh" here is a
# recording stand-in outside them (never the real one), run with a temp HOME; it must never be called.
OUT=$(mktemp -d /private/tmp/sr-outside.XXXXXX); trap 'rm -rf "$OUT" "$FX"' EXIT
printf '#!/bin/sh\necho "OUTSIDE $*" >> "%s/called"\nenv > "%s/env"\nexit 0\n' "$OUT" "$OUT" > "$OUT/gh"; chmod +x "$OUT/gh"
ln -s "$OUT/gh" "$FX/gh-link"
printf '#!/bin/sh\nexec "%s" "$@"\n' "$OUT/gh" > "$FX/gh-wrap"; chmod +x "$FX/gh-wrap"
for spec in "outside:$OUT/gh" "symlink:$FX/gh-link" "dotdot:$FX/../$(basename "$OUT")/gh"; do
  l=${spec%%:*} p=${spec#*:}; rm -f "$OUT/called"
  (cd "$REPO" && env HOME="$FX/ghome" SR_PUBLISH_DRY_GH="$p" SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$FX/npm" SR_PUBLISH_DRY_CURL="$FX/curl" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls2" \
    sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && [ ! -e "$OUT/called" ] && grep -q 'SR_PUBLISH_DRY_GH' "$FX/o" && pass "__publish-dry, a $l gh: refused for the gh stand-in, the outside gh never called" || fail "__publish-dry, $l: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1)"
done
# a stand-in whose name ends in a newline, next to a symlink without it: refused, the symlink's
# target never called (the checked name and the run name must be one string)
nl='
'
cp "$FX/gh" "$FX/ghn$nl"; ln -s "$OUT/gh" "$FX/ghn"; rm -f "$OUT/called"
(cd "$REPO" && env HOME="$FX/ghome" SR_PUBLISH_DRY_GH="$FX/ghn$nl" SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$FX/npm" SR_PUBLISH_DRY_CURL="$FX/curl" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls2" \
  sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
[ $r != 0 ] && [ ! -e "$OUT/called" ] && grep -q 'SR_PUBLISH_DRY_GH' "$FX/o" && pass "a stand-in name ending in a newline: refused for the gh stand-in, the symlink's target never called" || fail "newline name: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1)"
# a wrapper inside the fixtures that execs a gh outside them cannot be seen before it runs: what
# it reaches runs with a fresh temp HOME and no token or transport variable, so a real gh would
# have no login and could not write
rm -f "$OUT/called" "$OUT/env"
(cd "$REPO" && env HOME="$FX/ghome" GH_TOKEN="$DECOY" GH_CONFIG_DIR="$FX/ghcfg" SSH_AUTH_SOCK="$FX/sock" SR_PUBLISH_DRY_GH="$FX/gh-wrap" \
  SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$FX/npm" SR_PUBLISH_DRY_CURL="$FX/curl" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls2" sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1
if [ -e "$OUT/env" ]; then
  h=$(sed -n 's/^HOME=//p' "$OUT/env")
  case $h in /private/tmp/sr-*) ! grep -q -e '^GH_TOKEN=' -e '^GH_CONFIG_DIR=' -e '^SSH_AUTH_SOCK=' -e '^GITHUB_TOKEN=' "$OUT/env" \
      && pass "a wrapper reaching an outside gh: that gh ran with a temp HOME and no token or transport" || fail "wrapper: the outside gh got a token or transport" ;;
    *) fail "wrapper: the outside gh's HOME is '$h'" ;; esac
else fail "a wrapper reaching an outside gh: not reached, so its environment is not checked ($(tail -1 "$FX/o"))"; fi
printf '#!/bin/sh\necho "OUTSIDE npm $*" >> "%s/called"\nexit 0\n' "$OUT" > "$OUT/npm"; chmod +x "$OUT/npm"; ln -s "$OUT/npm" "$FX/npm-link"
for spec in "outside:$OUT/npm" "symlink:$FX/npm-link"; do
  l=${spec%%:*} p=${spec#*:}; rm -f "$OUT/called"
  (cd "$REPO" && env HOME="$FX/ghome" SR_PUBLISH_DRY_GH="$FX/gh" SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$p" SR_PUBLISH_DRY_CURL="$FX/curl" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls2" \
    sh scripts/release.sh __publish-dry --out "$FX/out" v0.1.1-rc.1) > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && [ ! -e "$OUT/called" ] && grep -q 'SR_PUBLISH_DRY_NPM' "$FX/o" && pass "__publish-dry, a $l npm: refused for the npm stand-in, the outside npm never called" || fail "__publish-dry, $l npm: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1) $(tail -1 "$FX/o")"
done
printf '#!/bin/sh\necho "OUTSIDE curl $*" >> "%s/called"\nexit 0\n' "$OUT" > "$OUT/curl"; chmod +x "$OUT/curl"; ln -s "$OUT/curl" "$FX/curl-link"
for spec in "outside:$OUT/curl" "symlink:$FX/curl-link"; do
  l=${spec%%:*} p=${spec#*:}
  for entry in __publish-dry __publish-cask-dry; do rm -f "$OUT/called"
    (cd "$REPO" && env HOME="$FX/ghome" SR_PUBLISH_DRY_GH="$FX/gh" SR_PUBLISH_DRY_GIT="$FX/git" SR_PUBLISH_DRY_NPM="$FX/npm" SR_PUBLISH_DRY_CURL="$p" SR_ASK_SCRIPT="$FX/ask" SR_ASK_RECORD="$FX/calls2" \
      sh scripts/release.sh $entry --out "$FX/out" v0.1.0) > "$FX/o" 2>&1; r=$?
    [ $r != 0 ] && [ ! -e "$OUT/called" ] && grep -q 'SR_PUBLISH_DRY_CURL' "$FX/o" && pass "$entry, a $l curl: refused for the curl stand-in, the outside curl never called" || fail "$entry, $l curl: rc=$r called=$(cat "$OUT/called" 2>/dev/null | head -1) $(tail -1 "$FX/o")"
  done
done
# the dry run's gh and git get a fresh temp HOME, and no GH_CONFIG_DIR or SSH_AUTH_SOCK: even a
# real gh that got this far would have no login
pub v0.1.1-rc.1 v0.1.1-rc.1
h=$(sed -n 's/^HOME=//p' "$FX/env.gh.0"); case $h in /private/tmp/sr-*) pass "the dry gh's HOME is a fresh temp dir ($h)" ;; *) fail "the dry gh's HOME is '$h'" ;; esac
grep -q -e '^GH_CONFIG_DIR=' -e '^SSH_AUTH_SOCK=' "$FX"/env.gh.* "$FX"/env.git.* && fail "the dry gh or git got GH_CONFIG_DIR or SSH_AUTH_SOCK" || pass "the dry gh and git get no GH_CONFIG_DIR or SSH_AUTH_SOCK"
# the list publishing reads again (the stamp does not bind it): the main package last, and all
# four names; a changed list is refused before any call at all
for bad in "sheepr sheepr-linux-arm64 sheepr-linux-x64 sheepr-darwin-universal" "sheepr-linux-arm64 sheepr-linux-x64 sheepr"; do
  sed -i.bak "s/^SR_NPM_PKGS=.*/SR_NPM_PKGS='$bad'/" "$REPO/scripts/release.conf" && rm -f "$REPO/scripts/release.conf.bak"
  pub v0.1.0 v0.1.0; r=$?; g checkout -q scripts/release.conf
  [ $r = 1 ] && [ ! -s "$FX/calls" ] && grep -q 'SR_NPM_PKGS' "$FX/o" && pass "a list of '$bad': refused before any call" || fail "list '$bad': rc=$r calls=$(head -1 "$FX/calls" 2>/dev/null) $(tail -1 "$FX/o")"
done
# an npm name that is not the owner's, or not on npm yet: refused before any git or gh call
nogh() { ! grep -q -e '^gh ' -e '^git ' "$FX/calls"; }
pub v0.1.0 v0.1.0 >/dev/null; h=$(sed -n 's/^HOME=//p' "$FX/env.npm.0" 2>/dev/null)
case $h in /private/tmp/sr-dryhome.*) ! grep -l "$DECOY" "$FX"/env.npm.* >/dev/null 2>&1 && pass "the dry run's npm: a temp HOME, no decoy token" || fail "the dry run's npm got a decoy token" ;; *) fail "the dry run's npm HOME is '$h'" ;; esac
echo stranger > "$FX/owner.sheepr"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/owner.sheepr"
[ $r = 1 ] && nogh && grep -q 'sheepr on npm belongs to stranger, not to lukasco' "$FX/o" && pass "an npm name owned by someone else: refused before any git or gh call" || fail "npm owner: rc=$r $(seq) $(tail -1 "$FX/o")"
: > "$FX/ownerfail.sheepr-linux-arm64"; pub v0.1.0 v0.1.0; r=$?; rm -f "$FX/ownerfail.sheepr-linux-arm64"
[ $r = 1 ] && nogh && grep -q 'sheepr-linux-arm64 is not on npm yet' "$FX/o" && pass "an npm name not on npm yet: refused before any git or gh call" || fail "npm absent: rc=$r $(seq) $(tail -1 "$FX/o")"
n=$(ls -d /private/tmp/sr-dryhome.* 2>/dev/null | wc -l | tr -d ' ')
[ "$n" = 0 ] && pass "no dry HOME is left, after the refused runs too" || fail "$n dry HOME dirs left"
finish
