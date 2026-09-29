#!/bin/sh
# PHASE3.md S2 (review): the build's order and its compiler pin, with cargo, rustc, docker and lipo
# as shims (nothing is compiled). A compiler that is not the tag's rust-toolchain.toml pin stops
# the build before any Linux step (exit 1, the pin named); a failing docker stops it at the Linux
# step, before any macOS bundle or signing step (a failure there must not cost a notarization).
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0-rc.1
pin=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$SD_ROOT/rust-toolchain.toml")
S=$FX/shims; mkdir -p "$S"
printf '#!/bin/sh\necho "cargo $*" >> "%s/calls"\nexit 0\n' "$S" > "$S/cargo"
printf '#!/bin/sh\necho "rustc $*" >> "%s/calls"\ncat "%s/rustc.v"\n' "$S" "$S" > "$S/rustc"
printf '#!/bin/sh\necho "docker $*" >> "%s/calls"\nexit 1\n' "$S" > "$S/docker"
printf '#!/bin/sh\necho "lipo $*" >> "%s/calls"\nexit 1\n' "$S" > "$S/lipo"
chmod +x "$S"/*
b() { rm -f "$S/calls"; rm -rf "$FX/out"
  (cd "$REPO" && env PATH="$S:$PATH" HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 sh scripts/release.sh build --out "$FX/out" v0.1.0-rc.1) > "$FX/o" 2>&1; }
echo "rustc 1.0.0 (000000000 2020-01-01)" > "$S/rustc.v"; b; r=$?
[ $r = 1 ] && grep -q "pinned $pin" "$FX/o" && ! grep -q '^docker' "$S/calls" && pass "a compiler off the pin: refused before any Linux step" || fail "off-pin: rc=$r $(tail -1 "$FX/o")"
echo "rustc $pin (000000000 2026-01-01)" > "$S/rustc.v"; b; r=$?
[ $r = 1 ] && grep -q 'step: the Linux builds' "$FX/o" && ! grep -q 'step: the macOS bundle' "$FX/o" && ! grep -q '^lipo' "$S/calls" \
  && pass "a failing docker: stopped at the Linux step, before the bundle" || fail "docker failing: rc=$r $(grep step "$FX/o" | tr '\n' ' ')"
[ "$(g worktree list | grep -c .)" = 1 ] && pass "no worktree left after a failed build" || fail "a worktree left"
finish
