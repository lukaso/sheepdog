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
    rel_fat_plist_arm64|rel_fat_plist_x86)
      fx_plist "$d/rel.plist" com.lukaso.sheepdog; fx_plist "$d/dev.plist" com.lukaso.sheepdog.dev
      printf 'int main(){return 0;}\n' > "$d/e.c"
      if [ "$1" = rel_fat_plist_arm64 ]; then ap=rel.plist xp=dev.plist; else ap=dev.plist xp=rel.plist; fi
      cc -arch arm64 -o "$d/a" "$d/e.c" -sectcreate __TEXT __info_plist "$d/$ap" || exit 3
      cc -arch x86_64 -o "$d/x" "$d/e.c" -sectcreate __TEXT __info_plist "$d/$xp" || exit 3
      lipo -create -output "$d/sheepdog" "$d/x" "$d/a" || exit 3; echo "$d/sheepdog" ;;
    rel_upper_ident) fx_prog "$d/sheepdog"; codesign -s - -f -i COM.LUKASO.SHEEPDOG "$d/sheepdog" 2>/dev/null; echo "$d/sheepdog" ;;
    rel_space_ident) fx_prog "$d/sheepdog"; codesign -s - -f -i "com.lukaso.sheepdog " "$d/sheepdog" 2>/dev/null; echo "$d/sheepdog" ;;
    rel_upper_bundle) fx_bundle "$d" COM.LUKASO.SHEEPDOG; echo "$d/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    rel_upper_embedded) fx_plist "$d/Info.plist" COM.LUKASO.SHEEPDOG
      printf 'int main(){return 0;}\n' > "$d/e.c"; cc -o "$d/sheepdog" "$d/e.c" -sectcreate __TEXT __info_plist "$d/Info.plist" || exit 3
      echo "$d/sheepdog" ;;
    rel_lowercase_path) fx_bundle "$d" com.lukaso.sheepdog; echo "$d/sheepdog.app/contents/macos/sheepdog" ;;
    rel_nested_bundle) mkdir -p "$d/Out.app/Contents/MacOS"; fx_plist "$d/Out.app/Contents/Info.plist" com.lukaso.sheepdog.dev
      fx_bundle "$d/Out.app/Contents/MacOS" com.lukaso.sheepdog
      echo "$d/Out.app/Contents/MacOS/Sheepdog.app/Contents/MacOS/sheepdog" ;;
    rel_shebang) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      printf '#!%s\necho hi\n' "$d/Sheepdog.app/Contents/MacOS/sheepdog" > "$d/script"; chmod +x "$d/script"; echo "$d/script" ;;
    rel_dir_symlink) mkdir -p "$d/Q.app/Contents" "$d/plain"; fx_plist "$d/Q.app/Contents/Info.plist" com.lukaso.sheepdog
      fx_prog "$d/plain/sheepdog"; ln -s ../../plain "$d/Q.app/Contents/MacOS"; echo "$d/Q.app/Contents/MacOS/sheepdog" ;;
    dev_script) printf '#!/bin/sh\necho hi\n' > "$d/script"; chmod +x "$d/script"; echo "$d/script" ;;
    dev_symlink) fx_bundle "$d" com.lukaso.sheepdog.dev; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      mkdir -p "$d/bin"; ln -s ../Sheepdog.app/Contents/MacOS/sheepdog "$d/bin/sheepdog"; echo "$d/bin/sheepdog" ;;
    rel_embedded_space_path|rel_embedded_tab_path)
      if [ "$1" = rel_embedded_space_path ]; then sub="a b"; else sub="$(printf 'a\tb')"; fi
      mkdir -p "$d/$sub"; fx_plist "$d/Info.plist" com.lukaso.sheepdog
      printf 'int main(){return 0;}\n' > "$d/e.c"; cc -o "$d/$sub/sheepdog" "$d/e.c" -sectcreate __TEXT __info_plist "$d/Info.plist" || exit 3
      echo "$d/$sub/sheepdog" ;;
    rel_fat_ident_x86|rel_fat_ident_arm64)
      printf 'int main(){return 0;}\n' > "$d/e.c"
      cc -arch arm64 -o "$d/a" "$d/e.c" && cc -arch x86_64 -o "$d/x" "$d/e.c" || exit 3
      if [ "$1" = rel_fat_ident_x86 ]; then ai=com.lukaso.sheepdog.dev xi=com.lukaso.sheepdog; else ai=com.lukaso.sheepdog xi=com.lukaso.sheepdog.dev; fi
      codesign -s - -f -i "$ai" "$d/a" 2>/dev/null; codesign -s - -f -i "$xi" "$d/x" 2>/dev/null
      lipo -create -output "$d/sheepdog" "$d/x" "$d/a" || exit 3; echo "$d/sheepdog" ;;
    rel_dot_path) mkdir -p "$d/Q.app/Contents" "$d/plain"; fx_plist "$d/Q.app/Contents/Info.plist" com.lukaso.sheepdog
      fx_prog "$d/plain/sheepdog"; ln -s ../../plain "$d/Q.app/Contents/MacOS"; echo "$d/Q.app/Contents/./MacOS/sheepdog" ;;
    rel_dot_relative) mkdir -p "$d/Q.app/Contents" "$d/plain"; fx_plist "$d/Q.app/Contents/Info.plist" com.lukaso.sheepdog
      fx_prog "$d/plain/sheepdog"; ln -s ../../plain "$d/Q.app/Contents/MacOS"; echo "$d/Q.app/Contents/MacOS/.//./sheepdog" ;;
    rel_flat_plist) fx_prog "$d/sheepdog"; fx_plist "$d/Info.plist" com.lukaso.sheepdog; echo "$d/sheepdog" ;;
    dev_flat_plist) fx_prog "$d/sheepdog"; fx_plist "$d/Info.plist" com.lukaso.sheepdog.dev; echo "$d/sheepdog" ;;
    rel_shebang_env) fx_bundle "$d" com.lukaso.sheepdog; codesign -s - -f "$d/Sheepdog.app" 2>/dev/null
      printf '#!/usr/bin/env %s\necho hi\n' "$d/Sheepdog.app/Contents/MacOS/sheepdog" > "$d/script"; chmod +x "$d/script"; echo "$d/script" ;;
    dev_shebang_env_sh) printf '#!/usr/bin/env sh\necho hi\n' > "$d/script"; chmod +x "$d/script"; echo "$d/script" ;;
    rel_embedded_newline)
      printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.lukaso.sheepdog&#10;</string></dict></plist>\n' > "$d/Info.plist"
      printf 'int main(){return 0;}\n' > "$d/e.c"; cc -o "$d/sheepdog" "$d/e.c" -sectcreate __TEXT __info_plist "$d/Info.plist" || exit 3
      echo "$d/sheepdog" ;;
    *) echo "no fixture $1" >&2; exit 3 ;;
  esac
}
