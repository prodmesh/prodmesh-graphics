#!/bin/bash
# See what a template actually looks like, without putting it on air.
#
#   tools/look.sh <template.json> [--set k=v ...]
#
# Renders one held frame and composites it over a dark stage grey, because a
# keyed graphic is mostly transparent and a PNG of one opens as white-on-white
# in every viewer. Writes <name>-over-dark.png next to it and prints the path.
#
# This exists because the alternative was inferring what a graphic looked like
# from solved rectangle numbers, which is how this repo ended up believing text
# wrapped when it did not. Look at the pixels.
set -euo pipefail
cd "$(dirname "$0")/.."
TPL="${1:?usage: tools/look.sh <template.json> [--set k=v ...]}"
shift
FONT="${PMG_LOOK_FONT:-/System/Library/Fonts/Helvetica.ttc}"
OUT="${TMPDIR:-/tmp}/pmg-look"
rm -rf "$OUT"; mkdir -p "$OUT"

cargo build --release -p pmg-cli 2>&1 | grep -E "^error" && exit 1

# --frames-only: no NDI sender, so this needs no network and no receiver.
./target/release/pmgctl take "$TPL" --font "$FONT" --frames-only --hold 0.3 \
  --png "$OUT" "$@" > "$OUT/take.log" 2>&1 || { tail -20 "$OUT/take.log"; exit 1; }

FRAME=$(ls "$OUT"/*hold*.png 2>/dev/null | head -1)
[ -n "$FRAME" ] || FRAME=$(ls "$OUT"/*.png | tail -1)

PMG_FRAME="$FRAME" PMG_DEST="$OUT/look.png" python3 tools/over_dark.py
echo "$OUT/look.png"
