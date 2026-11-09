# Shared helpers for the reference harness (sourced, not executed).
#
# The real Wallpaper Engine runs under Proton inside Steam's pressure-vessel
# container. To drive it (`-control openWallpaper ...`) we start a second
# wallpaper64.exe in the SAME container + wineserver by entering the namespaces
# of the running one. Its window is moved to a headless Hyprland output
# (RPREF) so captures never cover the user's real monitors.

set -euo pipefail

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$HARNESS_DIR")"
HEADLESS="${RPREF_OUTPUT:-RPREF}"
WORKSHOP="${WE_WORKSHOP_DIR:-$HOME/.local/share/Steam/steamapps/workshop/content/431960}"
TESTSET="${RUSTPAPER_TESTSET_DIR:-$HOME/.local/share/rustpaper-testset}/steamapps/workshop/content/431960"
WE_DIR="${WE_DIR:-$HOME/.local/share/Steam/steamapps/common/wallpaper_engine}"

# wallpaper_dir <id>: folder of a wallpaper (Steam Workshop or test corpus)
wallpaper_dir() {
    local d
    # a folder path works too (calibration scenes, unpacked projects)
    if [ -f "$1/project.json" ]; then (cd "$1" && pwd); return 0; fi
    for d in "$WORKSHOP/$1" "$TESTSET/$1"; do
        [ -f "$d/project.json" ] && { echo "$d"; return 0; }
    done
    return 1
}

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "$1 not installed"; }

# Starts WE through Steam when it died (some wallpapers crash it) and prints
# its pid once wallpaper64.exe is up. WE_AUTOSTART=0 disables this.
start_we() {
    [ "${WE_AUTOSTART:-1}" = 1 ] || return 1
    echo "Wallpaper Engine is not running: starting it through Steam" >&2
    steam -applaunch 431960 >/dev/null 2>&1 &
    local pid
    for _ in $(seq 1 120); do
        pid=$(pgrep -x wallpaper64.exe | head -1) && {
            sleep 15
            # keep WE's own windows (UI, dialogs) off the user's monitors
            hyprctl clients -j | python3 -c "import json,sys; [print(c['address']) for c in json.load(sys.stdin) if c['class'] == 'steam_app_431960' and not c['title'].startswith('rpref_')]" |
                while read -r a; do
                    hyprctl dispatch "hl.dsp.window.move({ window = 'address:$a', workspace = 'name:rpref', follow = false })" >/dev/null 2>&1
                done
            echo "$pid"; return 0; }
        sleep 1
    done
    return 1
}

# WE's crash dialog ("Error", same window class) is up.
we_crashed() {
    hyprctl clients -j | python3 -c "import json,sys; sys.exit(0 if any(c['class'] == 'steam_app_431960' and c['title'] == 'Error' for c in json.load(sys.stdin)) else 1)"
}

# WE marks `safemode: true` in its config.json while running and clears it on
# a clean exit; after a kill the next start shows a "Safe Start" dialog that
# blocks every -control command. Clear just that flag (what a clean exit
# does); a one-time backup is kept next to it.
clear_safe_start() {
    local cfg="$WE_DIR/config.json"
    [ -f "$cfg" ] || return 0
    [ -f "$cfg.rustpaper-backup" ] || cp "$cfg" "$cfg.rustpaper-backup"
    python3 - "$cfg" <<'PY'
import json, sys
p = sys.argv[1]
j = json.load(open(p, encoding="utf-8-sig"))
changed = False
for user in j.values():
    g = user.get("general", {}).get("user") if isinstance(user, dict) else None
    if isinstance(g, dict) and g.get("safemode"):
        g["safemode"] = False
        changed = True
if changed:
    json.dump(j, open(p, "w"), indent="\t")
PY
}

# Kills WE (crashed or wedged: windows that never open or close) and starts
# it again.
restart_we() {
    echo "restarting Wallpaper Engine" >&2
    pkill -x launcher.exe 2>/dev/null || true
    pkill -x wallpaperui.exe 2>/dev/null || true
    pkill -x wallpaper64.exe 2>/dev/null || true
    for _ in $(seq 1 30); do pgrep -x wallpaper64.exe >/dev/null || break; sleep 1; done
    clear_safe_start
    start_we >/dev/null || die "could not restart Wallpaper Engine"
}

# we_control <args for wallpaper64.exe...>
we_control() {
    local pid
    pid=$(pgrep -x wallpaper64.exe | head -1) || pid=$(start_we) ||
        die "Wallpaper Engine is not running and could not be started"
    local env=() e
    while IFS= read -r -d '' e; do
        case "$e" in WINESERVERSOCKET=* | WINELOADERNOEXEC=* | WINEPRELOADRESERVE=*) ;; *) env+=("$e") ;; esac
    done <"/proc/$pid/environ"
    # Z: (the host root) always exists; the S: library drive is only mapped
    # in some Proton sessions ("wine: failed to open S:\\...").
    nsenter -t "$pid" -U -m --preserve-credentials env -i "${env[@]}" \
        wine "$(wine_path "$WE_DIR/wallpaper64.exe")" "$@" >/dev/null 2>&1
}

# Linux path -> path as seen by Wine (Z: is the host root).
wine_path() { printf 'Z:%s' "${1//\//\\}"; }

# Creates the headless output (1920x1080, scale 1) if it does not exist yet.
# The session lock covers every output, the headless one included: captures
# would record the password prompt. Hyprland reports an active
# ext-session-lock as "LOCK" in a monitor's solitaryBlockedBy.
session_locked() {
    hyprctl -j monitors 2>/dev/null | python3 -c "import json,sys; sys.exit(0 if any('LOCK' in (m.get('solitaryBlockedBy') or []) for m in json.load(sys.stdin)) else 1)"
}

ensure_headless() {
    session_locked && die "the session is locked: WE captures would record the lock screen"
    if ! hyprctl monitors -j | python3 -c "import json,sys; sys.exit(0 if any(m['name']=='$HEADLESS' for m in json.load(sys.stdin)) else 1)"; then
        hyprctl output create headless "$HEADLESS" >/dev/null
        sleep 1
    fi
    hyprctl eval "hl.monitor({ output = '$HEADLESS', mode = '1920x1080@60', position = 'auto', scale = 1 })" >/dev/null
    # When the idle daemon powers the monitors off (DPMS), the headless output
    # stops rendering too: grim hangs and WE's window captures black. Wake
    # only the headless output; the user's monitors stay off.
    hyprctl dispatch "hl.dsp.dpms({ action = 'on', monitor = '$HEADLESS' })" >/dev/null 2>&1 || true
    # WE's windows open straight on the hidden workspace and never take focus
    # (they re-request it while rendering). Runtime only: gone on the next
    # Hyprland reload. Also moves WE's own UI there while a run is going on.
    if [ -z "${_WE_RULE_SET:-}" ]; then
        hyprctl eval "hl.window_rule({ name = 'rustpaper-harness-we', match = { class = 'steam_app_431960' }, workspace = 'name:rpref silent', no_focus = true, no_initial_focus = true, focus_on_activate = false })" >/dev/null 2>&1 || true
        export _WE_RULE_SET=1
    fi
}

remove_headless() { hyprctl output remove "$HEADLESS" >/dev/null 2>&1 || true; }

window_address() { # window_address <title>
    hyprctl clients -j | python3 -c "import json,sys; print(next((c['address'] for c in json.load(sys.stdin) if c['title']=='$1'),''))"
}

active_address() { hyprctl activewindow -j | python3 -c "import json,sys; print(json.load(sys.stdin).get('address',''))"; }

# Moves a window to its own workspace on the headless output, fullscreen,
# and gives focus back to whatever the user had focused.
# Pass the window to refocus as $3 when the caller knows it (a window that
# just opened usually has taken focus already).
park_window() { # park_window <address> <workspace name> [address to refocus]
    local addr=$1 ws=$2 me=${3:-}
    [ -n "$me" ] || me=$(active_address)
    hyprctl dispatch "hl.dsp.window.move({ window = 'address:$addr', workspace = 'name:$ws', follow = false })" >/dev/null
    hyprctl dispatch "hl.dsp.workspace.move({ workspace = 'name:$ws', monitor = '$HEADLESS' })" >/dev/null
    hyprctl dispatch "hl.dsp.focus({ workspace = 'name:$ws' })" >/dev/null
    hyprctl dispatch "hl.dsp.window.move({ window = 'address:$addr', workspace = 'name:$ws', follow = false })" >/dev/null
    hyprctl dispatch "hl.dsp.window.fullscreen({ window = 'address:$addr' })" >/dev/null
    refocus "$me" "${PARK_MONITOR:-}"
    return 0
}

# RFC 3339 time a WE capture started (for RUSTPAPER_CLOCK: clock texts must
# show the same time). Older captures: the video's mtime minus its length.
capture_clock() { # capture_clock <capture dir> <seconds>
    if [ -s "$1/we_start.txt" ]; then cat "$1/we_start.txt"
    else date -Iseconds -d "@$(( $(stat -c %Y "$1/we.mp4") - ${2:-8} - 2 ))"; fi
}

# Scene time at the start of a WE capture (see we-capture.sh); captures made
# before it was recorded used a 6 s warm-up.
capture_time() { cat "$1/we_time.txt" 2>/dev/null || echo 6; }

# Cursor position WE saw during a capture (0..1 inside its window, clamped);
# older captures did not record it: center.
capture_pointer() { cat "$1/we_pointer.txt" 2>/dev/null || echo 0.5,0.5; }

# rustpaper-render arguments replaying the audio WE heard during a capture
# (none for older captures: silence).
capture_audio() { [ -s "$1/we_audio.wav" ] && echo "--audio $1/we_audio.wav"; true; }

close_window() { hyprctl dispatch "hl.dsp.window.close({ window = 'address:$1' })" >/dev/null 2>&1 || true; }
# refocus <window address> [monitor]: back to the user's window, or at least
# their monitor when nothing was focused (empty workspace).
refocus() {
    if [ -n "${1:-}" ]; then
        hyprctl dispatch "hl.dsp.focus({ window = 'address:$1' })" >/dev/null 2>&1
    elif [ -n "${2:-}" ]; then
        hyprctl dispatch "hl.dsp.focus({ monitor = '$2' })" >/dev/null 2>&1
    fi
    return 0
}
# The user's monitor: the focused one unless that is the headless output,
# else the one under the cursor (focus can be left on RPREF by WE's windows).
focused_monitor() {
    hyprctl monitors -j | python3 -c "
import json, subprocess, sys
ms = json.load(sys.stdin)
f = next((m['name'] for m in ms if m['focused'] and m['name'] != '$HEADLESS'), '')
if not f:
    x, y = (float(v) for v in subprocess.run(['hyprctl', 'cursorpos'], capture_output=True, text=True).stdout.split(','))
    f = next((m['name'] for m in ms if m['name'] != '$HEADLESS' and m['x'] <= x < m['x'] + m['width'] / m['scale'] and m['y'] <= y < m['y'] + m['height'] / m['scale']), '')
print(f)"
}
