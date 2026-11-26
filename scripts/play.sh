#!/usr/bin/env bash
# Starts the title under Proton (Linux: SteamOS, CachyOS, ...) with the settings of
# scripts/play.ps1. NOT TESTED YET on Linux: say what happens (the log, the first error).
#
#   scripts/play.sh [--safe] [--ray-tracing] [--build diag|release] <your copy's executable> [VAR=value ...]
#
# Needs: a victory.exe built for Windows (on Windows, or on Linux with cargo-xwin, README.md),
# and umu-run (umu-launcher), or PROTON=<a Proton folder> with its `proton` script.
# Paths for the layer (victory.toml, VICTORY_GAME_DIR, ...) are Windows paths under Wine:
# /home/me/game is Z:\home\me\game.
set -euo pipefail

safe=0 rays=0 build=diag
while [[ $# -gt 0 ]]; do
    case "$1" in
        --safe) safe=1; shift ;;
        --ray-tracing) rays=1; shift ;;
        --build) build="$2"; shift 2 ;;
        *) break ;;
    esac
done
[[ $# -ge 1 ]] || { sed -n '2,10p' "$0"; exit 2; }
exe="$(realpath "$1")"; shift

here="$(cd "$(dirname "$0")/.." && pwd)"
victory="$here/target/x86_64-pc-windows-msvc/$build/victory.exe"
[[ -f "$victory" ]] || victory="$here/target/$build/victory.exe"
[[ -f "$victory" ]] || { echo "No victory.exe under target/. Build it first (README.md)."; exit 1; }
[[ -f "$exe" ]] || { echo "No $exe."; exit 1; }

# Other VICTORY_ settings left in this shell go; where your files are stays.
keep="VICTORY_CONFIG VICTORY_GAME_DIR VICTORY_CACHE_DIR VICTORY_DATA_DIR"
for v in $(compgen -e | grep '^VICTORY_' || true); do
    [[ " $keep " == *" $v "* ]] || unset "$v"
done

# The settings of scripts/play.ps1.
export VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1
export VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2
export VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0 VICTORY_CARD_AWAKE=1
export VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8 VICTORY_SKIP_PIPELINES=none
# (vkd3d-proton keeps no device-removed data: VICTORY_D3D12_DRED is left off here.)
if [[ $safe == 1 ]]; then
    export VICTORY_UNDER_WIDE=0 VICTORY_LAND_ECHO=0
else
    export VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1
fi
[[ $rays == 1 ]] && export VICTORY_BVH_BUILDER=1
for kv in "$@"; do export "${kv?}"; done

# Wine sees the root of the Linux file system as drive Z:.
winpath() { printf 'Z:%s' "${1//\//\\}"; }

cd "$here"   # victory.toml is looked for here
if command -v umu-run >/dev/null; then
    export GAMEID="${GAMEID:-umu-default}" WINEPREFIX="${WINEPREFIX:-$HOME/.local/share/victory/prefix}"
    exec umu-run "$victory" run "$(winpath "$exe")" --execute
elif [[ -n "${PROTON:-}" && -x "$PROTON/proton" ]]; then
    export STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/victory/compat}"
    export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.steam/steam}"
    mkdir -p "$STEAM_COMPAT_DATA_PATH"
    exec "$PROTON/proton" run "$victory" run "$(winpath "$exe")" --execute
else
    echo "Neither umu-run nor PROTON=<a Proton folder> found."; exit 1
fi
