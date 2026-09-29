#!/bin/sh
# Render install.sh for a release (PHASE3.md S3): write the version in, and embed the exec door
# (scripts/lib/exec-guard.sh, verbatim) and the release ID and requirement from release.conf, so the
# installer's door is the tested door, not a second copy.
#   render-install.sh SRC VERSION OUT
set -u
[ $# -eq 3 ] || { echo "usage: render-install.sh SRC VERSION OUT" >&2; exit 2; }
src=$1 ver=$2 outf=$3
lib=$(cd "$(dirname "$0")" && pwd -P) || exit 1
printf '%s\n' "$ver" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$' || { echo "render-install: bad version $ver" >&2; exit 2; }
grep -q '^SD_DOOR_EOF$' "$lib/exec-guard.sh" && { echo "render-install: the door holds the heredoc end marker" >&2; exit 1; }
[ "$(grep -c '^: @@SD_DOOR@@$' "$src")" = 1 ] && [ "$(grep -c '^SHEEPDOG_VERSION=$' "$src")" = 1 ] || { echo "render-install: $src lacks its markers" >&2; exit 1; }
conf=$(grep -E '^SD_RELEASE_(ID|REQUIREMENT)=' "$lib/../release.conf")
[ "$(printf '%s\n' "$conf" | grep -c .)" = 2 ] || { echo "render-install: release.conf lacks the release ID or requirement" >&2; exit 1; }
{
  while IFS= read -r line; do
    case $line in
      'SHEEPDOG_VERSION=') echo "SHEEPDOG_VERSION=$ver" ;;
      ': @@SD_DOOR@@')
        echo 'write_door() { # dir: the exec door (scripts/lib/exec-guard.sh) and its release.conf'
        echo '  mkdir -p "$1/lib" || return 1'
        echo "  cat > \"\$1/lib/exec-guard.sh\" <<'SD_DOOR_EOF'"
        cat "$lib/exec-guard.sh"
        echo 'SD_DOOR_EOF'
        echo "  cat > \"\$1/release.conf\" <<'SD_CONF_EOF'"
        printf '%s\n' "$conf"
        echo 'SD_CONF_EOF'
        echo '  chmod 755 "$1/lib/exec-guard.sh"'
        echo '}' ;;
      *) printf '%s\n' "$line" ;;
    esac
  done < "$src"
} > "$outf.tmp" && chmod 755 "$outf.tmp" && mv "$outf.tmp" "$outf"
