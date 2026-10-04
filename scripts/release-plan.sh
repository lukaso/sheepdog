#!/bin/sh
# The publish planner (PHASE3.md §1.2). Pure: it reads files and the local git, and prints the
# exact GitHub calls `release.sh publish` makes, or refuses.
#
#   release-plan.sh --out DIR --tag vTAG --remote LSREMOTE --releases RELEASES
#   release-plan.sh --validate PLAN
#
# LSREMOTE is `git ls-remote origin refs/tags/vTAG*` output; RELEASES the tag names of the
# repository's releases and drafts, one per line. Refused (exit 1):
#   - a manifest that is not this tag's signed, non-control build;
#   - an upload set that is not exactly the five release files (other files in DIR are allowed
#     and never uploaded);
#   - a SHA256SUMS that is not exactly four well-formed lines naming the other four, each matching;
#   - one of those four whose sha256 is not its MANIFEST.json entry's (the build's);
#   - a manifest commit that is not the local tag's commit, or not the remote tag's (for an
#     annotated tag only the peeled `^{}` line counts; no remote tag is refused);
#   - a release or draft that already uses the tag.
# The plan (one shape only, which --validate checks):
#   POST repos/lukaso/sheepr/releases -F draft=true [-F prerelease=true] -f tag_name=vTAG -f name=vTAG
#     (prerelease exactly for a vX.Y.Z-rc.N tag, so an rc is never the "latest" release)
#   UPLOAD <file>            (the five, in a fixed order)
#   PATCH -F draft=false
# `-F` sends a JSON boolean (`-f` would send the string "true"); there is no target_commitish: with
# the tag missing, GitHub would make it from the default branch when the draft is published.
set -u
PATH=/usr/bin:/bin:$PATH
LC_ALL=C; export LC_ALL
die() { echo "release-plan: $*" >&2; exit 1; }
usage() { echo "usage: release-plan.sh --out DIR --tag vTAG --remote FILE --releases FILE | --validate PLAN" >&2; exit 2; }
FIVE="sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh SHA256SUMS"
REPO=lukaso/sheepr

pre() { case $1 in *-rc.*) printf '%s' '-F prerelease=true ' ;; esac; }
validate() { # plan-file -> 0 if it is exactly the one shape
  f=$1
  t=$(sed -n 's/^POST repos\/lukaso\/sheepr\/releases -F draft=true \(-F prerelease=true \)\{0,1\}-f tag_name=\(v[0-9.a-z-]*\) -f name=\2$/\2/p' "$f")
  [ -n "$t" ] || die "the plan has no POST of the one shape"
  exp="POST repos/$REPO/releases -F draft=true $(pre "$t")-f tag_name=$t -f name=$t"
  for x in $FIVE; do exp="$exp
UPLOAD $x"; done
  exp="$exp
PATCH -F draft=false"
  [ "$(cat "$f")" = "$exp" ] || die "the plan is not of the one shape"
}

out="" tag="" remote="" releases=""
case ${1:-} in
  --validate) [ $# -eq 2 ] || usage; validate "$2"; exit 0 ;;
esac
while [ $# -gt 0 ]; do
  [ $# -ge 2 ] || usage
  case $1 in --out) out=$2 ;; --tag) tag=$2 ;; --remote) remote=$2 ;; --releases) releases=$2 ;; *) usage ;; esac
  shift 2
done
[ -n "$out" ] && [ -n "$tag" ] && [ -f "$remote" ] && [ -f "$releases" ] || usage
[ -d "$out" ] || die "no directory $out"
m=$out/MANIFEST.json
[ -f "$m" ] || die "no MANIFEST.json"
mv_() { sed -n "s/^ *\"$1\": *\"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}$/\1/p" "$m" | head -1; }
[ "$(mv_ tag)" = "$tag" ] || die "the manifest is for $(mv_ tag), not $tag"
[ "$(mv_ mode)" = signed ] || die "the manifest's mode is '$(mv_ mode)', not signed"
[ "$(mv_ control)" = false ] || die "the manifest is a control build"
# a control build's output nested anywhere below is refused too
nested=$(find "$out" -mindepth 2 -name MANIFEST.json -exec grep -l '"control": *true' {} + 2>/dev/null | head -1)
[ -z "$nested" ] || die "a control build's manifest lies inside $out ($nested)"
mc=$(mv_ commit)

# the upload set and SHA256SUMS
for x in $FIVE; do [ -f "$out/$x" ] || die "missing $x"; done
[ "$(grep -c '' "$out/SHA256SUMS")" = 4 ] || die "SHA256SUMS does not have exactly four lines"
names=$(sed 's/^[0-9a-f]\{64\}  //' "$out/SHA256SUMS" | sort | tr '\n' ' ')
[ "$names" = "install.sh sheepr-linux-aarch64 sheepr-linux-x86_64 sheepr-macos-universal.tar.gz " ] \
  || die "SHA256SUMS names $names"
(cd "$out" && shasum -a 256 -c --strict SHA256SUMS >/dev/null 2>&1) || die "a file does not match SHA256SUMS (or a malformed line)"
# and each is the file the build made: its MANIFEST.json hash (SHA256SUMS sits in the same
# directory, so a file changed with SHA256SUMS made again would pass the check above)
for x in sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh; do
  h=$(shasum -a 256 "$out/$x" | cut -d' ' -f1)
  [ "$(grep -c "\"name\": \"$x\", \"sha256\": \"$h\"" "$m")" = 1 ] || die "$x does not match its manifest hash"
done

# the commit: local and remote
lc=$(git rev-parse -q --verify "refs/tags/$tag^{commit}") || die "no local tag $tag"
[ "$mc" = "$lc" ] || die "the manifest's commit $mc is not $tag's $lc"
peeled=$(awk -v r="refs/tags/$tag^{}" '$2 == r {print $1}' "$remote")
plain=$(awk -v r="refs/tags/$tag" '$2 == r {print $1}' "$remote")
if [ -n "$peeled" ]; then rc=$peeled
elif [ -n "$plain" ] && [ "$plain" = "$lc" ]; then rc=$plain   # a lightweight tag names the commit
elif [ -n "$plain" ]; then die "the remote $tag is annotated but its peeled line is missing (the tag object is not the commit)"
else die "no remote tag $tag (push it first)"; fi
[ "$rc" = "$lc" ] || die "the remote $tag is on $rc, not $lc"
grep -qx "$tag" "$releases" && die "a release or draft already uses $tag"

tmp=$(mktemp "${TMPDIR:-/tmp}/sr-plan.XXXXXX") || exit 1
{
  echo "POST repos/$REPO/releases -F draft=true $(pre "$tag")-f tag_name=$tag -f name=$tag"
  for x in $FIVE; do echo "UPLOAD $x"; done
  echo "PATCH -F draft=false"
} > "$tmp"
validate "$tmp"
cat "$tmp"; rm -f "$tmp"
