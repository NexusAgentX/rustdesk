#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
python3 "$BASELINE_ROOT/apply-ci-config.py"
cd "$BASELINE_ROOT/source"
python3 build.py --flutter --hwcodec --unix-file-copy-paste --screencapturekit
