#!/bin/sh
# PHASE3.md S4: the Homebrew cask, rendered from packaging/homebrew/sheepdog.rb.in by
# scripts/lib/render-cask.sh (what release.sh build --sign runs): the version and the archive's
# sha256 are written in; the result is valid Ruby and passes `brew style` (offline); a bad version
# or hash is refused. `brew audit --cask` and the install itself are in the clean-user leg (they
# need a tapped tap).
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
R="$SD_ROOT/scripts/lib/render-cask.sh"
H=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
sh "$R" 0.1.0-rc.1 "$H" "$FX/sheepdog.rb" && pass "rendered" || fail "render failed"
grep -q '^  version "0.1.0-rc.1"$' "$FX/sheepdog.rb" && grep -q "^  sha256 \"$H\"\$" "$FX/sheepdog.rb" && ! grep -q '@' "$FX/sheepdog.rb" \
  && pass "the version and the hash are written in, no placeholder left" || fail "rendered: $(grep -e version -e sha256 "$FX/sheepdog.rb" | tr '\n' ' ')"
ruby -c "$FX/sheepdog.rb" >/dev/null 2>&1 && pass "valid Ruby" || fail "not valid Ruby"
if command -v brew >/dev/null 2>&1; then
  # outside a tap, brew style applies its own source rules (Sorbet sigils, a frozen-string
  # comment; --except-cops does not drop the Sorbet ones, measured), which casks in a tap do not
  # carry: every other offense fails the row
  HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_FROM_API=1 timeout 300 brew style "$FX/sheepdog.rb" > "$FX/style" 2>&1
  grep -q 'file inspected' "$FX/style" || fail "brew style did not run: $(tail -2 "$FX/style" | tr '\n' ' ')"
  other=$(grep -E ': [CWEF]: ' "$FX/style" | grep -v -e 'Sorbet/StrictSigil' -e 'Sorbet/TrueSigil' -e 'Style/FrozenStringLiteralComment')
  [ -z "$other" ] && pass "brew style: no cask offense" || fail "brew style: $other"
else echo "note: no brew here; brew style not run"; fi
for bad in "0.1 $H" "v0.1.0 $H" "0.1.0 xyz" "0.1.0 ${H}0"; do
  sh "$R" ${bad% *} ${bad#* } "$FX/bad.rb" >/dev/null 2>&1 && fail "accepted '$bad'" || pass "refused '$bad'"
done
finish
