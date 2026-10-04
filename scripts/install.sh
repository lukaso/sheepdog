#!/bin/sh
# Install sheepr (https://github.com/lukaso/sheepr).
#
#   curl -fsSL https://github.com/lukaso/sheepr/releases/latest/download/install.sh | sh
#
# macOS: installs Sheepr.app into ~/Applications (replacing an older one) and links
# ~/.local/bin/sheepr to its executable. The app must carry the project's Developer ID signature
# and pass Gatekeeper; anything else is refused and nothing is installed.
# Linux: installs the static binary into ~/.local/bin (or /usr/local/bin as root).
#
# Every download is checked against the release's SHA256SUMS. On Linux that proves the download is
# intact, not where it came from: SHA256SUMS comes from the same release. On macOS the Developer ID
# signature proves where it came from.
#
# SHEEPR_INSTALL_BASE overrides where the files come from: https, or http on 127.0.0.1, ::1 or
# localhost only.
#
# Exit: 0 installed; 1 a check or a step failed: nothing installed, unless the last message says
# what is in place (the new app with no link, or one that does not run; or no app, with the old
# one kept aside); 2 usage; 3 a refused SHEEPR_INSTALL_BASE.
set -u
SHEEPR_VERSION=
REPO=lukaso/sheepr

say() { echo "sheepr install: $*"; }
die() { echo "sheepr install: $*" >&2; exit 1; }

: @@SR_DOOR@@

[ -n "$SHEEPR_VERSION" ] || die "this is the unrendered install.sh; use the one from a release"
# HOME names where it installs (except as root on Linux)
if [ -z "${HOME:-}" ] && { [ "$(uname -s)" = Darwin ] || [ "$(id -u)" != 0 ]; }; then die "HOME is not set; it names where sheepr is installed"; fi
base=${SHEEPR_INSTALL_BASE:-https://github.com/$REPO/releases/download/v$SHEEPR_VERSION}

# the base: https, or http on exactly a loopback host (no userinfo, fragment, query or backslash
# before the host ends)
case $base in
  https://*) ;;
  http://*)
    hp=${base#http://}; hp=${hp%%/*}
    case $hp in *@*|*'#'*|*'?'*|*\\*) echo "sheepr install: refused base $base" >&2; exit 3 ;; esac
    case $hp in
      '['*']'|'['*']:'*) host=${hp%%]*}]; rest=${hp#"$host"} ;;
      *) host=${hp%%:*}; rest=${hp#"$host"} ;;
    esac
    case $rest in ''|:[0-9]*) ;; *) echo "sheepr install: refused base $base" >&2; exit 3 ;; esac
    case $rest in :*[!0-9]*) echo "sheepr install: refused base $base" >&2; exit 3 ;; esac
    case $host in 127.0.0.1|localhost|'[::1]') ;; *) echo "sheepr install: refused base $base (http only on loopback)" >&2; exit 3 ;; esac ;;
  *) echo "sheepr install: refused base $base (https, or http on loopback)" >&2; exit 3 ;;
esac
base=${base%/}

if command -v curl >/dev/null 2>&1; then fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then fetch() { wget -q -O "$2" "$1"; }
else die "needs curl or wget; install one of them"; fi
if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
else die "needs sha256sum or shasum"; fi

case $(uname -s) in
  Darwin) art=sheepr-macos-universal.tar.gz ;;
  Linux)
    case $(uname -m) in
      aarch64|arm64) art=sheepr-linux-aarch64 ;;
      x86_64|amd64) art=sheepr-linux-x86_64 ;;
      *) die "no release for Linux $(uname -m)" ;;
    esac ;;
  *) die "no release for $(uname -s)" ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sheepr-install.XXXXXX") || die "no temp dir"
stage="" old="" app=""
# on any exit or signal: if the old app was moved aside and no app is in place, put it back first
cleanup() {
  if [ -n "$old" ] && [ -e "$old" ] && [ -n "$app" ] && [ ! -e "$app" ]; then
    mv "$old" "$app" 2>/dev/null || { echo "sheepr install: the old app is at $old" >&2; stage=""; }
  fi
  rm -rf "$tmp"; [ -n "$stage" ] && rm -rf "$stage"
}
trap cleanup EXIT
trap 'cleanup; exit 1' HUP INT TERM

say "downloading $art ($SHEEPR_VERSION)"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "cannot download SHA256SUMS from $base"
fetch "$base/$art" "$tmp/$art" || die "cannot download $art from $base"
want=$(grep -E "^[0-9a-f]{64}  $art\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
[ "$(printf '%s\n' "$want" | grep -c .)" = 1 ] || die "SHA256SUMS has no single checksum line for $art"
[ "$(sha "$tmp/$art")" = "$want" ] || die "the checksum of $art does not match SHA256SUMS"

if [ "$(uname -s)" = Darwin ]; then
  apps=$HOME/Applications app=$HOME/Applications/Sheepr.app
  mkdir -p "$apps" || die "cannot make $apps"
  stage=$(mktemp -d "$apps/.sheepr-install.XXXXXX") || die "no staging dir in $apps"
  tar -xzf "$tmp/$art" -C "$stage" || die "cannot unpack $art"
  new=$stage/Sheepr.app
  [ -d "$new" ] || die "the archive holds no Sheepr.app"
  write_door "$tmp/door" || die "cannot write the exec door"
  . "$tmp/door/release.conf"
  /usr/bin/codesign -v -R="$SR_RELEASE_REQUIREMENT" "$new" >/dev/null 2>&1 \
    || die "Sheepr.app does not carry the project's Developer ID signature; nothing installed"
  /usr/sbin/spctl -a -t exec "$new" >/dev/null 2>&1 || die "Gatekeeper rejects Sheepr.app; nothing installed"
  /bin/sh -p "$tmp/door/lib/exec-guard.sh" check "$new/Contents/MacOS/sheepr" || die "Sheepr.app fails the last signature check; nothing installed"
  # two moves, not one atomic swap: between them there is no app, and a sheepr starting then
  # falls back as PLAN.md §4.4 says
  # old is set before the move: a signal during it finds cleanup knowing where the old app went
  if [ -e "$app" ]; then old=$stage/Sheepr.app.old; mv "$app" "$old" || { old=""; die "cannot move the old app aside"; }; fi
  if ! mv "$new" "$app"; then
    # put the old one back; if that fails too, keep it where it is and say where
    if [ -n "$old" ] && mv "$old" "$app"; then old=""; die "cannot move Sheepr.app into $apps (the old one is back in place)"; fi
    [ -n "$old" ] && { keep=$old; old=""; stage=""; die "cannot move Sheepr.app into $apps, nor the old one back; the old one is at $keep"; }
    die "cannot move Sheepr.app into $apps"
  fi
  old=""
  rm -rf "$stage"; stage=""
  bin=$HOME/.local/bin
  mkdir -p "$bin" && ln -sf "$app/Contents/MacOS/sheepr" "$bin/sheepr" \
    || die "Sheepr.app is installed in $apps, but $bin/sheepr cannot be linked to it (link it yourself, or remove what is in the way and run this again)"
  v=$(/bin/sh -p "$tmp/door/lib/exec-guard.sh" exec "$bin/sheepr" --version) || die "Sheepr.app is installed in $apps, but sheepr does not run (the lines above say why)"
else
  if [ "$(id -u)" = 0 ]; then bin=/usr/local/bin; else bin=$HOME/.local/bin; fi
  mkdir -p "$bin" || die "cannot make $bin"
  cp "$tmp/$art" "$bin/.sheepr.new.$$" && chmod 755 "$bin/.sheepr.new.$$" && mv "$bin/.sheepr.new.$$" "$bin/sheepr" \
    || { rm -f "$bin/.sheepr.new.$$"; die "cannot install into $bin"; }
  v=$("$bin/sheepr" --version) || die "sheepr is installed in $bin, but does not run (the lines above say why)"
fi
say "installed: $v"
# the line that puts BIN on PATH in the startup file the user's shell ($SHELL) reads. macOS's
# Terminal opens login shells: zsh (the default) reads ~/.zprofile and never ~/.profile; bash the
# first of ~/.bash_profile, ~/.bash_login, ~/.profile that exists. A Linux terminal window opens a
# non-login shell: zsh reads ~/.zshrc, bash ~/.bashrc (for a login bash the distributions' own
# ~/.profile reads ~/.bashrc). fish has its own command.
sr_path_hint() { # bin
  sh_=${SHELL:-}   # unset in some containers, and install.sh runs with set -u
  case ${sh_##*/} in
    zsh) f='~/.zshrc'; [ "$(uname -s)" = Darwin ] && f='~/.zprofile'
         echo "  echo 'export PATH=\"$1:\$PATH\"' >> $f" ;;
    bash) # a login bash (macOS's Terminal) reads only the first of these that exists
          f='~/.bashrc'
          if [ "$(uname -s)" = Darwin ]; then
            f='~/.bash_profile'
            for g in .bash_profile .bash_login .profile; do [ -e "$HOME/$g" ] && { f="~/$g"; break; }; done
          fi
          echo "  echo 'export PATH=\"$1:\$PATH\"' >> $f" ;;
    fish) echo "  fish_add_path \"$1\"" ;;
    *) echo "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.profile" ;;
  esac
}
case :${PATH:-}: in
  *:"$bin":*) ;;
  *) say "$bin is not on your PATH; add it (then open a new terminal), for example:"
     sr_path_hint "$bin" ;;
esac
