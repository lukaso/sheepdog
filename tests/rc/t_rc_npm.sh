#!/bin/sh
# PHASE3.md S5, cell 18 with the stapled bundle: tests/dist/t_npm_install.sh over the rc's own four
# npm packages (SD_RC_DIR, read only) in place of a test build.
exec env SD_NPM_RC_DIR="$SD_RC_DIR" sh "$(dirname "$0")/../dist/t_npm_install.sh"
