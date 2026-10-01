#!/bin/sh
# PHASE3.md S5, cell 18 with the stapled bundle: tests/dist/t_npm_install.sh over the rc's own four
# npm packages (SD_RC_DIR, read only) in place of a test build.
set -u
[ -d "${SD_RC_DIR:-}" ] || { echo "FAIL: SD_RC_DIR is not a directory (${SD_RC_DIR:-unset})"; exit 1; }
exec env SD_NPM_RC_DIR="$SD_RC_DIR" sh "$(dirname "$0")/../dist/t_npm_install.sh"
