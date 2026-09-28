# Helpers for the phase-3 shell cells (PHASE3.md §1). POSIX sh; sourced.
#
# Fixtures with the release bundle ID are built only under /private/tmp/sd-p3-fixtures.XXXXXX
# (Launch Services does not index it, PLAN.md §4.4) and are never executed: a run of a
# mismatching build with the release ID switches off the operator's real privacy grant.

SD_ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
FAILS=0
fail() { echo "FAIL: $*"; FAILS=$((FAILS + 1)); }
pass() { echo "ok: $*"; }
finish() { [ "$FAILS" -eq 0 ] && { echo "PASS $(basename "$0")"; exit 0; }; echo "RED $(basename "$0"): $FAILS"; exit 1; }

# fx_dir: a fresh fixture directory, removed at exit.
fx_dir() {
  FX=$(mktemp -d /private/tmp/sd-p3-fixtures.XXXXXX) || exit 3
  trap 'rm -rf "$FX"' EXIT
}

# a tiny program that only writes a marker (it never reads a protected folder)
fx_prog() { # out-path
  printf '#include <stdio.h>\nint main(int c,char**v){FILE*f=fopen("%s/ran","a");if(f){fputs(v[0],f);fputs("\\n",f);fclose(f);}return 0;}\n' "$FX" > "$FX/prog.c"
  cc -o "$1" "$FX/prog.c" || exit 3
}

fx_plist() { # path bundle-id
  cat > "$1" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$2</string>
<key>CFBundleExecutable</key><string>sheepdog</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PL
}

fx_bundle() { # dir bundle-id -> $dir/Sheepdog.app, its executable not signed further
  mkdir -p "$1/Sheepdog.app/Contents/MacOS"
  fx_plist "$1/Sheepdog.app/Contents/Info.plist" "$2"
  fx_prog "$1/Sheepdog.app/Contents/MacOS/sheepdog"
}

# fx_build NAME: builds the named fixture and prints the path to hand to the door
fx_build() {
  d="$FX/$1"; mkdir -p "$d"
  case $1 in
    dev_bundle) fx_bundle "$d" com.lukaso.sheepdog.dev; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null; echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    dev_bare) fx_prog "$d/sheepdog"; codesign -s - -f -i com.lukaso.sheepdog.dev "$d/sheepdog" 2>/dev/null; echo "$d/sheepdog" ;;
    plain_unsigned) fx_prog "$d/sheepdog"; codesign --remove-signature "$d/sheepdog" 2>/dev/null; echo "$d/sheepdog" ;;
    rel_adhoc_bundle) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null; echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    rel_adhoc_bare) fx_prog "$d/sheepdog"; codesign -s - -f -i com.lukaso.sheepdog "$d/sheepdog" 2>/dev/null; echo "$d/sheepdog" ;;
    rel_adhoc_copy) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      cp "$d/Sheepdog.app/Contents/MacOS/sheepdog" "$d/copy"; echo "$d/copy" ;;
    rel_adhoc_hardlink) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      ln "$d/Sheepdog.app/Contents/MacOS/sheepdog" "$d/hard"; echo "$d/hard" ;;
    rel_adhoc_symlink) fx_prog "$d/sheepdog"; codesign -s - -f -i com.lukaso.sheepdog "$d/sheepdog" 2>/dev/null
      ln -s "$d/sheepdog" "$d/link"; echo "$d/link" ;;
    rel_bundle_linker_signed) fx_bundle "$d" com.lukaso.sheepdog; echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    rel_bundle_unsigned) fx_bundle "$d" com.lukaso.sheepdog; codesign --remove-signature "$d/Sheepdog.app/Contents/MacOS/sheepdog" 2>/dev/null
      echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    rel_embedded_plist) fx_plist "$d/Info.plist" com.lukaso.sheepdog
      printf 'int main(){return 0;}\n' > "$d/e.c"; cc -o "$d/sheepdog" "$d/e.c" -sectcreate __TEXT __info_plist "$d/Info.plist" || exit 3
      echo "$d/sheepdog" ;;
    rel_bundle_symlink) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      mkdir -p "$d/bin"; ln -s "../Sheepdog.app/Contents/MacOS/sheepdog" "$d/bin/sheepdog"; echo "$d/bin/sheepdog" ;;
    rel_bundle_linker_symlink) fx_bundle "$d" com.lukaso.sheepdog
      mkdir -p "$d/bin"; ln -s "../Sheepdog.app/Contents/MacOS/sheepdog" "$d/bin/sheepdog"; echo "$d/bin/sheepdog" ;;
    rel_bundle_bad_plist) fx_bundle "$d" com.lukaso.sheepdog; printf 'not a plist' > "$d/Sheepdog.app/Contents/Info.plist"
      echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    *) echo "no fixture $1" >&2; exit 3 ;;
  esac
}
