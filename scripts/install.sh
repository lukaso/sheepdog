#!/bin/sh
# Install sheepdog (https://github.com/lukaso/sheepdog).
#
#   curl -fsSL https://github.com/lukaso/sheepdog/releases/latest/download/install.sh | sh
#
# macOS: installs Sheepdog.app into ~/Applications (replacing an older one) and links
# ~/.local/bin/sheepdog to its executable. The app must carry the project's Developer ID signature
# and pass Gatekeeper; anything else is refused and nothing is installed.
# Linux: installs the static binary into ~/.local/bin (or /usr/local/bin as root).
#
# Every download is checked against the release's SHA256SUMS. On Linux that proves the download is
# intact, not where it came from: SHA256SUMS comes from the same release. On macOS the Developer ID
# signature proves where it came from.
#
# SHEEPDOG_INSTALL_BASE overrides where the files come from: https, or http on 127.0.0.1, ::1 or
# localhost only.
#
# Exit: 0 installed; 1 a check or a step failed (nothing installed); 2 usage; 3 a refused
# SHEEPDOG_INSTALL_BASE.
set -u
SHEEPDOG_VERSION=
REPO=lukaso/sheepdog

say() { echo "sheepdog install: $*"; }
die() { echo "sheepdog install: $*" >&2; exit 1; }

: @@SD_DOOR@@

[ -n "$SHEEPDOG_VERSION" ] || die "this is the unrendered install.sh; use the one from a release"
# HOME names where it installs (except as root on Linux)
if [ -z "${HOME:-}" ] && { [ "$(uname -s)" = Darwin ] || [ "$(id -u)" != 0 ]; }; then die "HOME is not set; it names where sheepdog is installed"; fi
base=${SHEEPDOG_INSTALL_BASE:-https://github.com/$REPO/releases/download/v$SHEEPDOG_VERSION}

# the base: https, or http on exactly a loopback host (no userinfo, fragment, query or backslash
# before the host ends)
case $base in
  https://*) ;;
  http://*)
    hp=${base#http://}; hp=${hp%%/*}
    case $hp in *@*|*'#'*|*'?'*|*\\*) echo "sheepdog install: refused base $base" >&2; exit 3 ;; esac
    case $hp in
      '['*']'|'['*']:'*) host=${hp%%]*}]; rest=${hp#"$host"} ;;
      *) host=${hp%%:*}; rest=${hp#"$host"} ;;
    esac
    case $rest in ''|:[0-9]*) ;; *) echo "sheepdog install: refused base $base" >&2; exit 3 ;; esac
    case $rest in :*[!0-9]*) echo "sheepdog install: refused base $base" >&2; exit 3 ;; esac
    case $host in 127.0.0.1|localhost|'[::1]') ;; *) echo "sheepdog install: refused base $base (http only on loopback)" >&2; exit 3 ;; esac ;;
  *) echo "sheepdog install: refused base $base (https, or http on loopback)" >&2; exit 3 ;;
esac
base=${base%/}

if command -v curl >/dev/null 2>&1; then fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then fetch() { wget -q -O "$2" "$1"; }
else die "needs curl or wget; install one of them"; fi
if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
else die "needs sha256sum or shasum"; fi

case $(uname -s) in
  Darwin) art=sheepdog-macos-universal.tar.gz ;;
  Linux)
    case $(uname -m) in
      aarch64|arm64) art=sheepdog-linux-aarch64 ;;
      x86_64|amd64) art=sheepdog-linux-x86_64 ;;
      *) die "no release for Linux $(uname -m)" ;;
    esac ;;
  *) die "no release for $(uname -s)" ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sheepdog-install.XXXXXX") || die "no temp dir"
stage="" old="" app=""
# on any exit or signal: if the old app was moved aside and no app is in place, put it back first
cleanup() {
  if [ -n "$old" ] && [ -e "$old" ] && [ -n "$app" ] && [ ! -e "$app" ]; then
    mv "$old" "$app" 2>/dev/null || { echo "sheepdog install: the old app is at $old" >&2; stage=""; }
  fi
  rm -rf "$tmp"; [ -n "$stage" ] && rm -rf "$stage"
}
trap cleanup EXIT
trap 'cleanup; exit 1' HUP INT TERM

say "downloading $art ($SHEEPDOG_VERSION)"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "cannot download SHA256SUMS from $base"
fetch "$base/$art" "$tmp/$art" || die "cannot download $art from $base"
want=$(grep -E "^[0-9a-f]{64}  $art\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
[ "$(printf '%s\n' "$want" | grep -c .)" = 1 ] || die "SHA256SUMS has no single checksum line for $art"
[ "$(sha "$tmp/$art")" = "$want" ] || die "the checksum of $art does not match SHA256SUMS"

if [ "$(uname -s)" = Darwin ]; then
  apps=$HOME/Applications app=$HOME/Applications/Sheepdog.app
  mkdir -p "$apps" || die "cannot make $apps"
  stage=$(mktemp -d "$apps/.sheepdog-install.XXXXXX") || die "no staging dir in $apps"
  tar -xzf "$tmp/$art" -C "$stage" || die "cannot unpack $art"
  new=$stage/Sheepdog.app
  [ -d "$new" ] || die "the archive holds no Sheepdog.app"
  write_door "$tmp/door" || die "cannot write the exec door"
  . "$tmp/door/release.conf"
  /usr/bin/codesign -v -R="$SD_RELEASE_REQUIREMENT" "$new" >/dev/null 2>&1 \
    || die "Sheepdog.app does not carry the project's Developer ID signature; nothing installed"
  /usr/sbin/spctl -a -t exec "$new" >/dev/null 2>&1 || die "Gatekeeper rejects Sheepdog.app; nothing installed"
  /bin/sh -p "$tmp/door/lib/exec-guard.sh" check "$new/Contents/MacOS/sheepdog" || die "the exec door refuses Sheepdog.app; nothing installed"
  # two moves, not one atomic swap: between them there is no app, and a sheepdog starting then
  # falls back as PLAN.md §4.4 says
  # old is set before the move: a signal during it finds cleanup knowing where the old app went
  if [ -e "$app" ]; then old=$stage/Sheepdog.app.old; mv "$app" "$old" || { old=""; die "cannot move the old app aside"; }; fi
  if ! mv "$new" "$app"; then
    # put the old one back; if that fails too, keep it where it is and say where
    if [ -n "$old" ] && mv "$old" "$app"; then old=""; die "cannot move Sheepdog.app into $apps (the old one is back in place)"; fi
    [ -n "$old" ] && { keep=$old; old=""; stage=""; die "cannot move Sheepdog.app into $apps; the old app is at $keep"; }
    die "cannot move Sheepdog.app into $apps"
  fi
  old=""
  rm -rf "$stage"; stage=""
  bin=$HOME/.local/bin
  mkdir -p "$bin" && ln -sf "$app/Contents/MacOS/sheepdog" "$bin/sheepdog" || die "cannot link $bin/sheepdog"
  v=$(/bin/sh -p "$tmp/door/lib/exec-guard.sh" exec "$bin/sheepdog" --version) || die "the installed sheepdog does not run"
else
  if [ "$(id -u)" = 0 ]; then bin=/usr/local/bin; else bin=$HOME/.local/bin; fi
  mkdir -p "$bin" || die "cannot make $bin"
  cp "$tmp/$art" "$bin/.sheepdog.new.$$" && chmod 755 "$bin/.sheepdog.new.$$" && mv "$bin/.sheepdog.new.$$" "$bin/sheepdog" \
    || { rm -f "$bin/.sheepdog.new.$$"; die "cannot install into $bin"; }
  v=$("$bin/sheepdog" --version) || die "the installed sheepdog does not run"
fi
say "installed: $v"
# the line that puts BIN on PATH in the startup file the user's login shell ($SHELL) reads: a login
# zsh (macOS's default) reads ~/.zprofile and never ~/.profile; bash on macOS (Terminal opens login
# shells) the first of ~/.bash_profile, ~/.bash_login, ~/.profile that exists, and on Linux
# ~/.bashrc; fish has its own command
sd_path_hint() { # bin
  sh_=${SHELL:-}   # unset in some containers, and install.sh runs with set -u
  case ${sh_##*/} in
    zsh) echo "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.zprofile" ;;
    bash) # a login bash (macOS's Terminal) reads only the first of these that exists
          f='~/.bashrc'
          if [ "$(uname -s)" = Darwin ]; then
            f='~/.bash_profile'
            for g in .bash_profile .bash_login .profile; do [ -e "$HOME/$g" ] && { f="~/$g"; break; }; done
          fi
          echo "  echo 'export PATH=\"$1:\$PATH\"' >> $f" ;;
    fish) echo "  fish_add_path $1" ;;
    *) echo "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.profile" ;;
  esac
}
case :${PATH:-}: in
  *:"$bin":*) ;;
  *) say "$bin is not on your PATH; add it (then open a new terminal), for example:"
     sd_path_hint "$bin" ;;
esac
