#!/usr/bin/env bash
set -euo pipefail
export IBUS_ADDRESS="unix:path=$FVOCI_NATIVE_IME_SESSION/i"
ibus-daemon --panel=disable --config=/usr/libexec/ibus-memconf --xim --emoji-extension=disable --cache=none \
  --address="$IBUS_ADDRESS" >"$FVOCI_NATIVE_IME_SESSION/ibus.log" 2>&1 &
ime_pid=$!
hangul_pid=''
cleanup() {
  if [[ -n "$hangul_pid" ]]; then
    kill "$hangul_pid" 2>/dev/null || true
    wait "$hangul_pid" 2>/dev/null || true
  fi
  kill "$ime_pid" 2>/dev/null || true
  wait "$ime_pid" 2>/dev/null || true
}
trap cleanup EXIT
ready=false
for attempt in {1..25}; do
  if timeout 2 ibus list-engine >"$FVOCI_NATIVE_IME_SESSION/engines.txt" 2>"$FVOCI_NATIVE_IME_SESSION/ibus-cli.log"; then
    ready=true; break
  fi
  sleep 0.1
done
[[ "$ready" == true ]]
/usr/libexec/ibus-engine-hangul --ibus >"$FVOCI_NATIVE_IME_SESSION/hangul.log" 2>&1 &
hangul_pid=$!
sleep 0.2
timeout 20 ibus engine hangul
ibus engine >"$FVOCI_NATIVE_IME_SESSION/selected-engine.txt"
[[ "$(cat "$FVOCI_NATIVE_IME_SESSION/selected-engine.txt")" == hangul ]]
printf 'display=%s\nbus=%s\nibus_pid=%s\nhangul_pid=%s\n' "$DISPLAY" "$DBUS_SESSION_BUS_ADDRESS" "$ime_pid" "$hangul_pid" >"$FVOCI_NATIVE_IME_SESSION/session.txt"
bun "$(dirname "$0")/../../../apps/web/e2e-native-ime/fixture-processes.ts"
"$@"
