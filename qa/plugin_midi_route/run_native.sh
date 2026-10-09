#!/usr/bin/env bash
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
export VST3_VALIDATION_ROOT="${VST3_VALIDATION_ROOT:-$(dirname "$here")}" TMPDIR="$here/tmp"
export HOME="$here/home" XDG_CONFIG_HOME="$here/home/config" XDG_DATA_HOME="$here/home/data"
mkdir -p "$TMPDIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
# The ordinary matrix excludes cold FX. The separate FX recovery test requires and
# verifies the cold latency-change fence, then validates the actual restarted chain.
export NATIVE_SKIP_FX=1 RUST_BACKTRACE=1
"$here/bin/native-route-tests" native_vst3_ --nocapture --test-threads=1
