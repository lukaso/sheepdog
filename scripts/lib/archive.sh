#!/bin/sh
# The macOS release archive (PHASE3.md §1.5).
#
#   archive.sh make APP OUT.tar.gz          tar APP (Sheepr.app) with no xattrs, ACLs or mac
#                                           metadata, owners 0/0 with no names
#   archive.sh check ARCHIVE [--signed|--signed-unstapled]
#                                           exit 0 only if ARCHIVE holds exactly the bundle
#
# The default macOS tar stores com.apple.provenance and any com.apple.quarantine in pax headers
# that `tar -t` does not show, and extraction restores them; it also stores the user's name.
# check: the regular files are exactly Sheepr.app/Contents/Info.plist and .../MacOS/sheepr
# (with --signed also .../_CodeSignature/CodeResources and the staple ticket .../CodeResources;
# with --signed-unstapled, the control mode's, the signature and no ticket),
# the directories exactly their parents, every entry owned 0/0 with no names, no xattr pax header,
# and no xattr on anything after extraction.
set -u
PATH=/usr/bin:/bin; export PATH
usage() { echo "usage: archive.sh make APP OUT.tar.gz | archive.sh check ARCHIVE [--signed|--signed-unstapled]" >&2; exit 2; }
bad() { echo "archive: $*" >&2; exit 1; }
[ $# -ge 2 ] || usage
case $1 in
  make)
    [ $# -eq 3 ] || usage
    app=$2 outf=$3
    [ "$(basename "$app")" = Sheepr.app ] && [ -d "$app" ] || bad "not a Sheepr.app: $app"
    (cd "$(dirname "$app")" && tar --no-xattrs --no-mac-metadata --no-acls --uid 0 --gid 0 --uname '' --gname '' \
      -czf "$outf" Sheepr.app) || bad "tar failed"
    ;;
  check)
    arc=$2 signed=no
    [ $# -le 3 ] || usage
    [ $# -eq 3 ] && case $3 in --signed) signed=yes ;; --signed-unstapled) signed=unstapled ;; *) usage ;; esac
    [ -f "$arc" ] || bad "no archive $arc"
    t=$(mktemp -d /private/tmp/sr-archive.XXXXXX 2>/dev/null || mktemp -d) || exit 1
    trap 'rm -rf "$t"' EXIT
    trap 'rm -rf "$t"; exit 1' HUP INT TERM
    want="Sheepr.app/Contents/Info.plist
Sheepr.app/Contents/MacOS/sheepr"
    [ $signed != no ] && want="$want
Sheepr.app/Contents/_CodeSignature/CodeResources"
    [ $signed = yes ] && want="$want
Sheepr.app/Contents/CodeResources"
    want=$(printf '%s\n' "$want" | sort)
    wantdirs=$(printf '%s\n' "$want" | while IFS= read -r f; do d=$(dirname "$f"); while [ "$d" != . ]; do echo "$d/"; d=$(dirname "$d"); done; done | sort -u)
    # the listing: regular files and directories, owners
    tar -tvzf "$arc" > "$t/list" 2>/dev/null || bad "cannot list $arc"
    files=$(awk '$1 ~ /^-/ {print $NF}' "$t/list" | sort)
    dirs=$(awk '$1 ~ /^d/ {print $NF}' "$t/list" | sed 's#/*$#/#' | sort -u)
    others=$(awk '$1 !~ /^[-d]/' "$t/list")
    [ -z "$others" ] || bad "entries that are neither files nor directories: $others"
    [ "$files" = "$want" ] || bad "the files are not exactly the bundle's: $(printf '%s' "$files" | tr '\n' ' ')"
    [ "$dirs" = "$wantdirs" ] || bad "the directories are not exactly the files' parents: $(printf '%s' "$dirs" | tr '\n' ' ')"
    owners=$(awk '{print $3 " " $4}' "$t/list" | sort -u)
    [ "$owners" = "0 0" ] || bad "owners are not 0/0 without names: $(printf '%s' "$owners" | tr '\n' ',')"
    gzip -dc "$arc" | grep -a -q -e 'SCHILY.xattr' -e 'LIBARCHIVE.xattr' && bad "an xattr pax header"
    mkdir "$t/x" && tar -xzf "$arc" -C "$t/x" || bad "cannot extract"
    # macOS adds com.apple.provenance to every file this process creates (measured: even
    # `echo > f`), so that one is the extraction's own; any other (quarantine, ...) came from the
    # archive. The pax-header check above is the one that sees what the archive carries.
    ex=$(xattr -lr "$t/x" 2>/dev/null | grep -v ': com.apple.provenance:' || true)
    [ -z "$ex" ] || bad "xattrs after extraction: $ex"
    ;;
  *) usage ;;
esac
