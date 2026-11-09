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

# we_control <args for wallpaper64.exe...>
we_control() {
    local pid
    pid=$(pgrep -x wallpaper64.exe | head -1) ||
        die "Wallpaper Engine is not running (start it from Steam first)"
    local env=() e
    while IFS= read -r -d '' e; do
        case "$e" in WINESERVERSOCKET=* | WINELOADERNOEXEC=* | WINEPRELOADRESERVE=*) ;; *) env+=("$e") ;; esac
    done <"/proc/$pid/environ"
    nsenter -t "$pid" -U -m --preserve-credentials env -i "${env[@]}" \
        wine 'S:\steamapps\common\wallpaper_engine\wallpaper64.exe' "$@" >/dev/null 2>&1
}

# Linux path -> path as seen by Wine (Z: is the host root).
wine_path() { printf 'Z:%s' "${1//\//\\}"; }

# Creates the headless output (1920x1080, scale 1) if it does not exist yet.
ensure_headless() {
    if ! hyprctl monitors -j | python3 -c "import json,sys; sys.exit(0 if any(m['name']=='$HEADLESS' for m in json.load(sys.stdin)) else 1)"; then
        hyprctl output create headless "$HEADLESS" >/dev/null
        sleep 1
    fi
    hyprctl eval "hl.monitor({ output = '$HEADLESS', mode = '1920x1080@60', position = 'auto', scale = 1 })" >/dev/null
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
    [ -n "$me" ] && hyprctl dispatch "hl.dsp.focus({ window = 'address:$me' })" >/dev/null
    return 0
}

close_window() { hyprctl dispatch "hl.dsp.window.close({ window = 'address:$1' })" >/dev/null 2>&1 || true; }
refocus() { [ -n "${1:-}" ] && hyprctl dispatch "hl.dsp.focus({ window = 'address:$1' })" >/dev/null 2>&1; return 0; }
