# The read-only Apple tools, by absolute path, each with its one subcommand (PHASE3.md §1.3).
# Sourced by release.sh (verify, publish, npm-check). A shim on PATH cannot answer for them, and
# DEVELOPER_DIR is unset for xcrun (it follows DEVELOPER_DIR to any tool). The requirement comes
# from release.conf, never from a caller. Each takes only a path.
#   rt_meets PATH        the Developer ID release requirement holds for PATH
#   rt_staple_ok PATH    the staple ticket validates
#   rt_spctl_ok PATH     Gatekeeper accepts PATH for execution
#   rt_cdhash PATH       prints "arm64=<cdhash> x86_64=<cdhash>" for PATH, or nothing unless both
#                        slices have one (`codesign -d` prints CDHash only at -vvv, measured)
rt_meets() { /usr/bin/codesign -v -R="$SD_RELEASE_REQUIREMENT" "$1" >/dev/null 2>&1; }
rt_staple_ok() { env -u DEVELOPER_DIR -u SDKROOT -u TOOLCHAINS /usr/bin/xcrun stapler validate "$1" >/dev/null 2>&1; }
rt_spctl_ok() { /usr/sbin/spctl -a -t exec "$1" >/dev/null 2>&1; }
rt_cdhash() {
  rt_a=$(/usr/bin/codesign -d -vvv --arch arm64 "$1" 2>&1 | /usr/bin/sed -n 's/^CDHash=//p')
  rt_x=$(/usr/bin/codesign -d -vvv --arch x86_64 "$1" 2>&1 | /usr/bin/sed -n 's/^CDHash=//p')
  [ -n "$rt_a" ] && [ -n "$rt_x" ] && echo "arm64=$rt_a x86_64=$rt_x"
}
