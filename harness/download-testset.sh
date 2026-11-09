#!/usr/bin/env bash
# download-testset.sh — downloads the test corpus (harness/testset.tsv) with
# steamcmd into ~/.local/share/rustpaper-testset (NOT the Steam client's
# Workshop folder: Steam deletes items you aren't subscribed to from there).
#
# Default: the RUNNING Steam client downloads each item without subscribing
# (`steam +workshop_download_item 431960 <id>`, uses your logged-in session)
# into its Workshop folder; the item is then copied here.
#
# Alternative with steamcmd (STEAM_USER set; interactive login once):
#   yay -S steamcmd            # or Valve's tarball
#   steamcmd +login <steam user> +quit
# Then:
#   STEAM_USER=<steam user> harness/download-testset.sh
#
# Wallpaper Engine must be owned by that account (Workshop content of a paid
# app can't be downloaded anonymously). Re-running only fetches what's missing.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="${RUSTPAPER_TESTSET_DIR:-$HOME/.local/share/rustpaper-testset}"
STEAM_CONTENT="${WE_WORKSHOP_DIR:-$HOME/.local/share/Steam/steamapps/workshop/content/431960}"
CONTENT="$DEST/steamapps/workshop/content/431960"
mkdir -p "$DEST"
mapfile -t IDS < <(grep -v '^#' "$HERE/testset.tsv" | cut -f1)

missing() {
    for id in "${IDS[@]}"; do
        [ -f "$CONTENT/$id/project.json" ] || echo "$id"
    done
}

copy_from_client() {
    local id=$1
    if [ -f "$STEAM_CONTENT/$id/project.json" ]; then
        mkdir -p "$CONTENT"
        rm -rf "${CONTENT:?}/$id.tmp" && cp -r "$STEAM_CONTENT/$id" "$CONTENT/$id.tmp" \
            && mv "$CONTENT/$id.tmp" "$CONTENT/$id"
    fi
}

if [ -z "${STEAM_USER:-}" ]; then
    pgrep -x steam >/dev/null || { echo "start the Steam client (or set STEAM_USER for steamcmd)" >&2; exit 1; }
    for attempt in 1 2 3; do
        mapfile -t TODO < <(missing)
        [ ${#TODO[@]} -eq 0 ] && break
        echo "attempt $attempt: ${#TODO[@]} item(s) via the Steam client"
        for id in "${TODO[@]}"; do
            if [ ! -f "$STEAM_CONTENT/$id/project.json" ]; then
                steam "+workshop_download_item 431960 $id" >/dev/null 2>&1 || true
                # wait until the item lands (big items take a while)
                for _ in $(seq 1 300); do
                    [ -f "$STEAM_CONTENT/$id/project.json" ] && break
                    sleep 1
                done
            fi
            copy_from_client "$id"
            echo "  $id: $([ -f "$CONTENT/$id/project.json" ] && echo ok || echo missing)"
        done
    done
else
    STEAMCMD="$(command -v steamcmd || echo "$HOME/.local/share/steamcmd/steamcmd.sh")"
    [ -x "$STEAMCMD" ] || { echo "steamcmd not found (yay -S steamcmd)" >&2; exit 1; }
    for attempt in 1 2 3; do
        mapfile -t TODO < <(missing)
        [ ${#TODO[@]} -eq 0 ] && break
        echo "attempt $attempt: ${#TODO[@]} item(s) to download"
        # batches keep one slow item from stalling everything
        for ((i = 0; i < ${#TODO[@]}; i += 20)); do
            args=(+force_install_dir "$DEST" +login "$STEAM_USER")
            for id in "${TODO[@]:i:20}"; do
                args+=(+workshop_download_item 431960 "$id")
            done
            "$STEAMCMD" "${args[@]}" +quit || true
        done
    done
fi

mapfile -t LEFT < <(missing)
echo "done: $(( ${#IDS[@]} - ${#LEFT[@]} ))/${#IDS[@]} in $CONTENT"
[ ${#LEFT[@]} -eq 0 ] || { printf 'still missing: %s\n' "${LEFT[*]}"; exit 1; }
