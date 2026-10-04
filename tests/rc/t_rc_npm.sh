#!/bin/sh
# PHASE3.md S5, cell 18 with the stapled bundle: tests/dist/t_npm_install.sh over the rc's own four
# npm packages (SR_RC_DIR, read only) in place of a test build.
set -u
[ -d "${SR_RC_DIR:-}" ] || { echo "FAIL: SR_RC_DIR is not a directory (${SR_RC_DIR:-unset})"; exit 1; }
exec env SR_NPM_RC_DIR="$SR_RC_DIR" sh "$(dirname "$0")/../dist/t_npm_install.sh"
