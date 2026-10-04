#!/bin/sh
# install.sh's PATH hint (sr_path_hint, read from the rendered install.sh): the line it prints puts
# ~/.local/bin on PATH in the startup file the user's shell ($SHELL) really reads. On macOS
# (Terminal opens login shells): zsh ~/.zprofile; bash the first of ~/.bash_profile,
# ~/.bash_login, ~/.profile that exists (else ~/.bash_profile). On Linux (a terminal window opens a
# non-login shell): zsh ~/.zshrc, bash ~/.bashrc. fish its own command, its path one argument; any
# other shell, or SHELL unset under install.sh's set -u (dash), ~/.profile. Premise rows, with the
# real shells and a temp HOME: a login zsh reads ~/.zprofile and not ~/.profile (the old hint's
# file); a login bash reads ~/.bash_profile and not ~/.bashrc, and after the printed line is
# applied where only ~/.profile exists it keeps that file's settings; an interactive non-login zsh
# reads ~/.zshrc and not ~/.zprofile, and bash ~/.bashrc. Nothing is installed and no sheepr runs.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
I=$FX/install.sh
sh "$SR_ROOT/scripts/lib/render-install.sh" "$SR_ROOT/scripts/install.sh" 0.1.0 "$I" || { fail "render"; finish; }
sed -n '/^sr_path_hint() {/,/^}/p' "$I" > "$FX/hint.sh"
[ -s "$FX/hint.sh" ] || { fail "no sr_path_hint in install.sh"; finish; }
mkdir -p "$FX/lx"; printf '#!/bin/sh\necho Linux\n' > "$FX/lx/uname"; chmod 755 "$FX/lx/uname"
hint() { # SHELL [linux|HOME-with-files] -> the printed hint (HOME: a fresh empty dir unless given)
  if [ "${2:-}" = linux ]; then p=$FX/lx:/usr/bin:/bin; else p=/usr/bin:/bin; fi
  case ${2:-} in /*) hh=$2 ;; *) hh=$(mktemp -d "$FX/hh.XXXXXX") ;; esac
  env -i PATH="$p" HOME="$hh" SHELL="$1" sh -c '. "$1"; sr_path_hint "$2"' sh "$FX/hint.sh" '$HOME/.local/bin'
}
row() { # what got want-substring
  case $2 in *"$3"*) pass "the hint for $1 uses $3" ;; *) fail "the hint for $1: '$2' (wanted $3)" ;; esac
}
row zsh "$(hint /bin/zsh)" '>> ~/.zprofile'
# on Linux a terminal window starts a non-login shell: zsh reads ~/.zshrc there (and a login zsh
# reads it too), never ~/.zprofile
row "zsh on Linux" "$(hint /bin/zsh linux)" '>> ~/.zshrc'
row "bash on macOS" "$(hint /bin/bash)" '>> ~/.bash_profile'
# a login bash reads only the first of ~/.bash_profile, ~/.bash_login, ~/.profile: the hint names the
# first that exists, so it never makes a file that hides another
mkdir -p "$FX/bp1" "$FX/bp2" "$FX/bp3"; : > "$FX/bp1/.profile"; : > "$FX/bp2/.bash_login"; : > "$FX/bp2/.profile"; : > "$FX/bp3/.bash_profile"; : > "$FX/bp3/.profile"
row "bash on macOS, only ~/.profile" "$(hint /bin/bash "$FX/bp1")" '>> ~/.profile'
row "bash on macOS, ~/.bash_login and ~/.profile" "$(hint /bin/bash "$FX/bp2")" '>> ~/.bash_login'
row "bash on macOS, ~/.bash_profile and ~/.profile" "$(hint /bin/bash "$FX/bp3")" '>> ~/.bash_profile'
row "bash on Linux" "$(hint /bin/bash linux)" '>> ~/.bashrc'
row fish "$(hint /opt/homebrew/bin/fish)" 'fish_add_path'
# a bin path with a space: the printed fish line, pasted, passes exactly one argument equal to it
# (read here with sh's quoting, which agrees with fish's for a space)
f=$(env -i PATH=/usr/bin:/bin HOME=/x SHELL=/opt/homebrew/bin/fish sh -c '. "$1"; sr_path_hint "/s p/.local/bin"' sh "$FX/hint.sh" | sed 's/^  //')
a1=$(sh -c 'fish_add_path() { printf "%s|%s" "$#" "$1"; }; eval "$1"' sh "$f")
[ "$a1" = "1|/s p/.local/bin" ] && pass "the fish line for a path with a space is one argument" || fail "the fish line for a path with a space: '$f' gives '$a1'"
row "sh" "$(hint /bin/sh)" '>> ~/.profile'
row "no SHELL" "$(hint '')" '>> ~/.profile'
# SHELL unset (a container's shell; dash and busybox ash do not fill it in, unlike bash) under
# install.sh's set -u: the hint must still print, not abort the install's last line
# (/bin/dash, as a container's /bin/sh), and the script goes on after it
if [ -x /bin/dash ]; then
  u=$(env -i PATH=/usr/bin:/bin /bin/dash -c 'set -u; [ -z "${SHELL+x}" ] || exit 9; . "$1"; sr_path_hint /x/bin; echo after' dash "$FX/hint.sh" 2>&1); r=$?
  [ $r = 0 ] && case $u in *'>> ~/.profile'*after) true ;; *) false ;; esac && pass "the hint with SHELL unset under set -u (dash): ~/.profile, and the script goes on" || fail "the hint with SHELL unset under set -u (dash): rc=$r '$u'"
else fail "no /bin/dash: the SHELL-unset row did not run (macOS ships it; on Linux dist-linux.sh covers SHELL unset)"
fi
# premise: what each login shell reads, with the hint's own line applied in a temp HOME
login_path() { # shell file -> PATH of a login shell after the hint's line went into FILE
  h=$(mktemp -d "$FX/h.XXXXXX"); mkdir -p "$h/.local/bin"
  printf 'export PATH="%s/.local/bin:$PATH"\n' "$h" > "$h/$2"
  env -i HOME="$h" PATH=/usr/bin:/bin TERM=dumb "$1" -l -c 'printf %s "$PATH"' 2>/dev/null < /dev/null
  printf '|%s' "$h"
}
rc_path() { # shell file -> PATH of an interactive non-login shell after the hint's line went into FILE
  h=$(mktemp -d "$FX/h.XXXXXX"); mkdir -p "$h/.local/bin"
  printf 'export PATH="%s/.local/bin:$PATH"\n' "$h" > "$h/$2"
  env -i HOME="$h" PATH=/usr/bin:/bin TERM=dumb perl -MPOSIX -e 'POSIX::setsid() != -1 or die; exec @ARGV' "$1" -i -c 'printf %s "$PATH"' 2>/dev/null < /dev/null
  printf '|%s' "$h"
}
if [ -x /bin/zsh ]; then
  r=$(login_path /bin/zsh .zprofile); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) pass "premise: a login zsh reads ~/.zprofile" ;; *) fail "premise: a login zsh did not read ~/.zprofile (${r%|*})" ;; esac
  r=$(login_path /bin/zsh .profile); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) fail "premise: a login zsh read ~/.profile (the old hint would have worked)" ;; *) pass "premise: a login zsh does not read ~/.profile" ;; esac
  # a terminal window on Linux: an interactive zsh that is not a login shell reads ~/.zshrc, not ~/.zprofile
  r=$(rc_path /bin/zsh .zshrc); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) pass "premise: an interactive non-login zsh reads ~/.zshrc" ;; *) fail "premise: an interactive non-login zsh did not read ~/.zshrc (${r%|*})" ;; esac
  r=$(rc_path /bin/zsh .zprofile); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) fail "premise: an interactive non-login zsh read ~/.zprofile" ;; *) pass "premise: an interactive non-login zsh does not read ~/.zprofile" ;; esac
fi
if [ -x /bin/bash ]; then
  # with only ~/.profile, the hint's line applied as printed: a login bash keeps ~/.profile's settings
  h=$(mktemp -d "$FX/hb.XXXXXX"); mkdir -p "$h/.local/bin"; echo 'export MYTOOL=kept' > "$h/.profile"
  line=$(hint /bin/bash "$h" | sed 's/^  //'); env -i HOME="$h" PATH=/usr/bin:/bin sh -c "$line"
  r=$(env -i HOME="$h" PATH=/usr/bin:/bin TERM=dumb /bin/bash -l -c 'printf "%s|%s" "${MYTOOL:-unset}" "$PATH"' 2>/dev/null < /dev/null)
  case $r in "kept|"*"$h/.local/bin"*) pass "premise: after the hint, a login bash keeps ~/.profile's settings and has the PATH entry" ;; *) fail "premise: after the hint, a login bash has '$r'" ;; esac
  r=$(login_path /bin/bash .bash_profile); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) pass "premise: a login bash reads ~/.bash_profile" ;; *) fail "premise: a login bash did not read ~/.bash_profile (${r%|*})" ;; esac
  # a terminal window on Linux: an interactive bash that is not a login shell reads ~/.bashrc (a
  # login bash does not; the distributions' own ~/.profile reads it then)
  r=$(rc_path /bin/bash .bashrc); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) pass "premise: an interactive non-login bash reads ~/.bashrc" ;; *) fail "premise: an interactive non-login bash did not read ~/.bashrc (${r%|*})" ;; esac
  r=$(login_path /bin/bash .bashrc); h=${r##*|}; case ${r%|*} in *"$h/.local/bin"*) fail "premise: a login bash read ~/.bashrc" ;; *) pass "premise: a login bash does not read ~/.bashrc" ;; esac
fi
finish
