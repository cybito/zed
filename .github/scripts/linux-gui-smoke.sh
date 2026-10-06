#!/usr/bin/env bash
set -euo pipefail

export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

openbox --sm-disable > "$ZED_SMOKE_DIAGNOSTICS/window-manager.log" 2>&1 &
wm_pid=$!
sleep 1
kill -0 "$wm_pid"
"$ZED_SMOKE_BINARY" --user-data-dir "$ZED_SMOKE_FIXTURE/data" "$ZED_SMOKE_FIXTURE/smoke.txt" > "$ZED_SMOKE_DIAGNOSTICS/gui.log" 2>&1 &
pid=$!
trap 'kill "$pid" "$wm_pid" 2>/dev/null || true' EXIT

colors=0
for attempt in {1..90}; do
  xwininfo -root -tree > "$ZED_SMOKE_DIAGNOSTICS/window.txt"
  if grep -q smoke.txt "$ZED_SMOKE_DIAGNOSTICS/window.txt"; then
    window_id=$(awk '/"smoke.txt"/ { print $1; exit }' "$ZED_SMOKE_DIAGNOSTICS/window.txt")
    [[ -n $window_id ]] || { echo 'smoke window has no X11 ID' >&2; exit 1; }
    xdotool windowactivate --sync "$window_id"
    import -window "$window_id" "$ZED_SMOKE_DIAGNOSTICS/window.png"
    colors=$(identify -format '%k' "$ZED_SMOKE_DIAGNOSTICS/window.png")
    if [[ $colors =~ ^[0-9]+$ ]] && (( colors > 16 )); then
      zed_log="$ZED_SMOKE_FIXTURE/data/logs/Zed.log"
      if [[ -f $zed_log ]]; then cp "$zed_log" "$ZED_SMOKE_DIAGNOSTICS/Zed.log"; fi
      printf 'visible Zed Linux window captured (%s unique colors)\n' "$colors"
      exit 0
    fi
  fi
  kill -0 "$pid"
  sleep 1
done

zed_log="$ZED_SMOKE_FIXTURE/data/logs/Zed.log"
if [[ -f $zed_log ]]; then cp "$zed_log" "$ZED_SMOKE_DIAGNOSTICS/Zed.log"; fi
echo "no visible smoke.txt window after 90 attempts ($colors unique colors)" >&2
exit 1
