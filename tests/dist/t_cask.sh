#!/bin/sh
# PHASE3.md S4: the Homebrew cask, rendered from packaging/homebrew/sheepdog.rb.in by
# scripts/lib/render-cask.sh (what release.sh build runs). The rendered file is evaluated in Ruby
# with a stand-in `cask` DSL that records each stanza, and each stanza is checked against what the
# release makes: the url, with the version resolved, is the published archive (one of the five
# files publish uploads); `app` is the archive's top directory and the `binary` target an
# executable file in an archive made by archive.sh; the zap list is sheepdog's own state dir only;
# `depends_on macos` matches the bundle's LSMinimumSystemVersion. A bad version or hash (one line
# or several) is refused with nothing written. No brew command runs here: `brew style` installs
# gems into the operator's Homebrew, so it and `brew audit` run in the clean-user leg.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
R="$SD_ROOT/scripts/lib/render-cask.sh"
H=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
V=0.1.0-rc.1
sh "$R" "$V" "$H" "$FX/sheepdog.rb" && pass "rendered" || { fail "render failed"; finish; }
ruby -c "$FX/sheepdog.rb" >/dev/null 2>&1 && pass "valid Ruby" || fail "not valid Ruby"

# the stanzas, recorded by a stand-in DSL (appdir is the literal APPDIR)
cat > "$FX/dsl.rb" <<'RB'
require "json"
$r = { "unknown" => [] }
class Rec
  def version(v = nil) = v ? ($r["version"] = v) : $r["version"]
  def sha256(v) = $r["sha256"] = v
  def url(u, **_) = $r["url"] = u
  def name(*) = nil
  def desc(*) = nil
  def homepage(h) = $r["homepage"] = h
  def depends_on(**h) = ($r["depends_on"] ||= {}).merge!(h.transform_keys(&:to_s))
  def app(a, **_) = ($r["app"] ||= []) << a
  def binary(b, **_) = ($r["binary"] ||= []) << b
  def appdir = "APPDIR"
  def zap(**h) = ($r["zap"] ||= {}).merge!(h.transform_keys(&:to_s))
  def caveats(s = nil) = $r["caveats"] = s
  def method_missing(m, *_, **_, &_b) = $r["unknown"] << m.to_s
  def respond_to_missing?(*) = true
end
def cask(n, &b) = ($r["token"] = n; Rec.new.instance_eval(&b))
load ARGV[0]
puts JSON.generate($r)
RB
ruby "$FX/dsl.rb" "$FX/sheepdog.rb" > "$FX/stanzas.json" 2>"$FX/dsl.err" || { fail "the cask did not evaluate: $(head -2 "$FX/dsl.err")"; finish; }

# what the release makes: the archive (archive.sh, around a .dev bundle that is never run), the
# bundle's minimum macOS, and the five files publish uploads
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -o "$FX/bin" "$FX/m.c" || exit 3
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/bin" "$FX/b" 0.1.0 1) || exit 3
"$SD_ROOT/scripts/lib/archive.sh" make "$app" "$FX/a.tar.gz" || exit 3
tar -tvzf "$FX/a.tar.gz" > "$FX/list" || exit 3
min=$(/usr/bin/plutil -extract LSMinimumSystemVersion raw -o - "$app/Contents/Info.plist") || exit 3
five=$(sed -n 's/^FIVE="\(.*\)"$/\1/p' "$SD_ROOT/scripts/release-plan.sh")
[ -n "$five" ] || { fail "no upload set in release-plan.sh"; finish; }

python3 - "$FX/stanzas.json" "$FX/list" "$V" "$H" "$min" "$five" > "$FX/rows" <<'PY'
import sys, json
st, lst, v, h, mn, five = sys.argv[1:7]
r = json.load(open(st)); rows = open(lst).read().splitlines(); five = five.split()
def check(ok, msg): print(("ok: " if ok else "FAIL: ") + msg)
check(r.get("unknown") == [], "no stanza outside the checked set (%s)" % r.get("unknown"))
check(r.get("version") == v and r.get("sha256") == h, "version and sha256 written in (%s, %s)" % (r.get("version"), r.get("sha256")))
want = "https://github.com/lukaso/sheepdog/releases/download/v%s/sheepdog-macos-universal.tar.gz" % v
check(r.get("url") == want, "the url is the release's archive (%s)" % r.get("url"))
check(r.get("url", "").rsplit("/", 1)[-1] in five, "the url's file is one publish uploads (%s)" % five)
tops = sorted({x.split()[-1].split("/")[0] for x in rows})
check(r.get("app") == ["Sheepdog.app"] and tops == ["Sheepdog.app"], "app is the archive's top directory (%s, archive %s)" % (r.get("app"), tops))
b = r.get("binary") or [""]
rel = b[0][len("APPDIR/"):] if len(b) == 1 and b[0].startswith("APPDIR/") else None
check(rel is not None and any(x.startswith("-rwx") and x.split()[-1] == rel for x in rows),
      "the binary target is an executable in the archive (%s)" % b)
check(r.get("zap") == {"trash": "~/.local/state/sheepdog"}, "zap removes sheepdog's state dir only (%s)" % r.get("zap"))
names = {"12": "monterey", "13": "ventura", "14": "sonoma", "15": "sequoia", "26": "tahoe"}
check(r.get("depends_on") == {"macos": ">= :%s" % names.get(mn.split(".")[0], "?")},
      "depends_on macos matches LSMinimumSystemVersion %s (%s)" % (mn, r.get("depends_on")))
PY
[ $? = 0 ] || fail "the stanza check did not run"
cat "$FX/rows"
n=$(grep -c '^ok' "$FX/rows"); [ "$n" = 8 ] || fail "$n stanza rows passed, not 8"
FAILS=$((FAILS + $(grep -c '^FAIL' "$FX/rows")))

for bad in "0.1 $H" "v0.1.0 $H" "0.1.0 xyz" "0.1.0 ${H}0"; do
  rm -f "$FX/bad.rb" "$FX/bad.rb.tmp"
  sh "$R" ${bad% *} ${bad#* } "$FX/bad.rb" >/dev/null 2>&1 && fail "accepted '$bad'" || pass "refused '$bad'"
done
nl='
'
for c in "version|0.1.0${nl}0.1.0|$H" "sha256|0.1.0|$H${nl}$H"; do
  what=${c%%|*} rest=${c#*|}; bv=${rest%%|*} bh=${rest#*|}
  rm -f "$FX/bad.rb" "$FX/bad.rb.tmp"
  sh "$R" "$bv" "$bh" "$FX/bad.rb" > "$FX/o" 2>&1; r=$?
  [ $r != 0 ] && grep -q "bad $what" "$FX/o" && [ ! -e "$FX/bad.rb" ] && [ ! -e "$FX/bad.rb.tmp" ] \
    && pass "a two-line $what: refused by its own check, nothing written" || fail "a two-line $what: rc=$r $(head -1 "$FX/o"), files: $(ls "$FX" | grep bad.rb | tr '\n' ' ')"
done
finish
