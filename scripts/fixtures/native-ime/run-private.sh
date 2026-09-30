#!/usr/bin/env bash
# Explicit opt-in native XTest -> IBus Hangul fixture; never a default CI lane.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
: "${FVOCI_NATIVE_IME_EVIDENCE:?Set a task-owned evidence directory}"
mkdir -p "$FVOCI_NATIVE_IME_EVIDENCE"
mkdir -p "$ROOT/.ime"
export FVOCI_NATIVE_IME_SESSION
FVOCI_NATIVE_IME_SESSION="$(mktemp -d "$ROOT/.ime/XXXXXX")"
printf '%s\n' "$FVOCI_NATIVE_IME_SESSION" >>"$FVOCI_NATIVE_IME_EVIDENCE/sessions.txt"
mkdir -m 700 "$FVOCI_NATIVE_IME_SESSION"/{runtime,config,cache,data}
export XDG_RUNTIME_DIR="$FVOCI_NATIVE_IME_SESSION/runtime"
export XDG_CONFIG_HOME="$FVOCI_NATIVE_IME_SESSION/config"
export XDG_CACHE_HOME="$FVOCI_NATIVE_IME_SESSION/cache"
export XDG_DATA_HOME="$FVOCI_NATIVE_IME_SESSION/data"
export TMPDIR="$FVOCI_NATIVE_IME_SESSION/runtime"
export XDG_SESSION_TYPE=x11 GTK_IM_MODULE=ibus QT_IM_MODULE=ibus XMODIFIERS=@im=ibus
export GSETTINGS_BACKEND=keyfile
export PLAYWRIGHT_BROWSERS_PATH=/home/kinesis/.cache/ms-playwright
unset DISPLAY WAYLAND_DISPLAY XAUTHORITY DBUS_SESSION_BUS_ADDRESS IBUS_ADDRESS
cat >"$FVOCI_NATIVE_IME_SESSION/bus.conf" <<EOF
<busconfig><type>session</type><listen>unix:path=$FVOCI_NATIVE_IME_SESSION/b</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>
EOF
dbus-run-session --config-file="$FVOCI_NATIVE_IME_SESSION/bus.conf" -- \
  xvfb-run -a -f "$FVOCI_NATIVE_IME_SESSION/Xauthority" \
  -e "$FVOCI_NATIVE_IME_SESSION/xvfb.log" -s '-screen 0 1280x1024x24 -nolisten tcp' \
  bash "$ROOT/scripts/fixtures/native-ime/session.sh" "$@"
