#!/usr/bin/env bash
# Screenshot of the app inside a headless nested KWin, independent of the real desktop
# (KWin sends no frames to hidden windows, which would stall a test on the live session).
#
#   scripts/ui-screenshot.sh out.png [delay-seconds] [app arguments…]
#
# Test hooks read from the environment: STLB_POINTER=x,y, STLB_SEARCH=query, and the usual
# XDG_CONFIG_HOME / XDG_DATA_HOME / XDG_CACHE_HOME to keep test state away from real settings.
set -euo pipefail
out=$(realpath -m "$1")
delay=${2:-8}
shift $(( $# >= 2 ? 2 : $# ))
app=${STLB_APP:-$(realpath "$(dirname "$0")/../target/release/stl-browser")}
session=$(mktemp --suffix=.sh)
trap 'rm -f "$session"' EXIT
{
    echo '#!/bin/sh'
    printf 'exec %q' "$app"
    [ $# -gt 0 ] && printf ' %q' "$@"
    echo
} > "$session"
chmod +x "$session"
export STLB_SCREENSHOT="$out" STLB_SCREENSHOT_DELAY="$delay"
env -u DISPLAY timeout $(( ${delay%.*} + 60 )) \
    kwin_wayland --virtual --width 1400 --height 860 --socket "stlb-test-$$" --no-lockscreen \
    --exit-with-session "$session" 2>&1 | grep -E 'stl_browser|panicked' || true
test -f "$out" && echo "screenshot: $out"
