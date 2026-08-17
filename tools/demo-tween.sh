#!/bin/bash
# M3g on real SDI: a lower third rebinding live, the box growing and shrinking
# to fit each new line of a verse.
#
#   tools/demo-tween.sh                 the Canyon Hills 29:30 case
#   tools/demo-tween.sh --tween 900     slower, to watch the morph itself
#   tools/demo-tween.sh --tween 0       hard cuts, for comparison
#
# NEEDS a DeckLink with sub-device 1 free. Watch the card's output.
#
# The sections deliberately vary a lot in length, because the whole point is
# the box tracking the text rather than a fixed plate with text poured into it.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD
API=http://127.0.0.1:8519
TWEEN=600
HOLD=4
MINUTES=0        # --minutes N loops the verse so you can go and find the cable
while [ $# -gt 0 ]; do
  case "$1" in
    --tween) TWEEN="$2"; shift 2 ;;
    --hold)  HOLD="$2"; shift 2 ;;
    --minutes) MINUTES="$2"; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

RUN="${TMPDIR:-/tmp}/pmg-tween-$(date +%H%M%S)"
mkdir -p "$RUN"
say() { printf "\n\033[1m== %s\033[0m\n" "$*"; }
api() {
  local out code
  out=$(curl -s -w '\n%{http_code}' "$@")
  code=$(printf '%s' "$out" | tail -1)
  out=$(printf '%s' "$out" | sed '$d')
  if [ "$code" -ge 400 ]; then printf '   HTTP %s: %s\n' "$code" "$out" >&2; return 1; fi
  printf '%s' "$out"
}

say "0. no stragglers"
lsof -ti:8519 -sTCP:LISTEN | xargs -r kill 2>/dev/null || true
pkill -f "target/release/pmg-node" 2>/dev/null || true
sleep 2

say "1. build"
cargo build --release -p pmg-node -p pmg-cli 2>&1 | grep -E "^error" && exit 1

say "2. server"
PMG_DB="$RUN/var/pmg.db" PMG_CAS="$RUN/var/cas" node server/index.js > "$RUN/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; kill ${NODE_PID:-0} 2>/dev/null || true' EXIT
for _ in $(seq 1 40); do curl -sf "$API/api/health" >/dev/null 2>&1 && break; sleep 0.5; done

say "3. publish the lower third"
"$REPO/target/release/pmgctl" scene fixture "$RUN/third.json" >/dev/null
python3 - "$RUN/third.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["id"] = "01JTWEENDEMOLOWERTHIRD01"
d["name"] = "Tween demo"
for f in d.get("fonts", []):
    f.pop("asset", None)
# The fixture ellipsizes the name at 40 characters, which would flatten exactly
# the thing this demo exists to show: both strings truncate to the same width
# and the box never moves. Found the hard way.
def widen(n):
    c = n.get("content") or {}
    b = c.get("binding") or {}
    if "transform" in b:
        b["transform"] = [t for t in b["transform"]
                          if not (isinstance(t, dict) and "ellipsize" in t)]
    for ch in n.get("children", []):
        widen(ch)
widen(d["root"])
json.dump(d, open(sys.argv[1], "w"), indent=2)
PY
"$REPO/target/release/pmgctl" scene compile "$RUN/third.json" "$RUN/third.pmgscene" >/dev/null
TID=$(python3 -c "import json;print(json.load(open('$RUN/third.manifest.json'))['template_id'])")
api -X POST "$API/api/templates" -H 'Content-Type: application/json' \
  -d "{\"id\":\"$TID\",\"name\":\"Tween demo\"}" >/dev/null
python3 - "$RUN/third.pmgscene" "$RUN/third.manifest.json" <<'PY' > "$RUN/pub.json"
import base64, json, sys
print(json.dumps({"scene": base64.b64encode(open(sys.argv[1],'rb').read()).decode(),
                  "manifest": json.load(open(sys.argv[2]))}))
PY
REV=$(api -X POST "$API/api/templates/$TID/publish" -H 'Content-Type: application/json' \
  --data-binary @"$RUN/pub.json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["template_rev"])')
ITEM=$(api -X POST "$API/api/items" -H 'Content-Type: application/json' -d "$(python3 - <<PY
import json
print(json.dumps({"label": "Verse", "template_rev": "$REV",
                  "data": {"name": "Genesis 1:1", "title": "Reading"},
                  "destination": {"output": "out-0", "layer": "Lower Third"}}))
PY
)" | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')

say "4. node on the card"
rm -rf "$RUN/node"
"$REPO/target/release/pmg-node" --state-dir "$RUN/node" --server ws://127.0.0.1:8519 \
  --name tween --backend decklink --decklink 1 --decklink-mode "1080p59.94" \
  --layers "Fullscreen,Lower Third,Bug" \
  --font /System/Library/Fonts/Helvetica.ttc > "$RUN/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 60); do
  NID=$(api "$API/api/nodes" | python3 -c 'import sys,json;n=json.load(sys.stdin)["nodes"];print(n[0]["node_id"] if n else "")' || true)
  [ -n "$NID" ] && break
  sleep 1
done
[ -n "$NID" ] || { tail -15 "$RUN/node.log"; exit 1; }
api -X POST "$API/api/nodes/$NID/adopt" -H 'Content-Type: application/json' -d '{"adopted":true}' >/dev/null

cue() {  # <op> [name] [title]
  # Defaulted, because `take` passes one argument and `set -u` turns a bare $2
  # into a fatal error — which silently sent nothing to the card at all.
  local op="$1" nm="${2:-}" ti="${3:-}"
  PMG_OP="$op" PMG_NAME="$nm" PMG_TITLE="$ti" PMG_NID="$NID" \
  PMG_ITEM="$ITEM" PMG_TWEEN="$TWEEN" python3 - > "$RUN/cue.json" <<'PYCUE'
import json, os, uuid
op = os.environ["PMG_OP"]
body = {"node": os.environ["PMG_NID"], "cue_id": str(uuid.uuid4()), "op": op,
        "output": "out-0", "layer": "Lower Third", "deadline_ms": 2000}
if op == "take":
    body["item"] = os.environ["PMG_ITEM"]
if op == "update":
    # Through the environment rather than substituted into the source: a verse
    # with an apostrophe would otherwise be a syntax error.
    body["data"] = {"name": os.environ["PMG_NAME"], "title": os.environ["PMG_TITLE"]}
    body["tween_ms"] = int(os.environ["PMG_TWEEN"])
print(json.dumps(body))
PYCUE
  api -X POST "$API/api/cue" -H 'Content-Type: application/json' --data-binary @"$RUN/cue.json"
}

say "5. on air"
for attempt in $(seq 1 40); do cue take >/dev/null 2>&1 && break; sleep 2; done
sleep "$HOLD"

say "6. the verse, section by section — WATCH THE BOX"
step() {
  printf "   %-52s " "$1"
  if R=$(cue update "$1" "$2" 2>&1); then echo "morphing (${TWEEN}ms)"; else echo "REFUSED: $R"; fi
  sleep "$HOLD"
}
verse() {
  # Lengths vary a lot on purpose — the short ones next to the long ones are
  # where the box visibly travels.
  step "In the beginning God created"                  "Genesis 1:1"
  step "the heavens and the earth."                    "Genesis 1:1"
  step "Now the earth was formless and empty"          "Genesis 1:2"
  step "and there was light."                          "Genesis 1:3"
  step "darkness was over the surface of the deep"     "Genesis 1:2"
  step "Let there be light"                            "Genesis 1:3"
}

if [ "$MINUTES" -gt 0 ]; then
  echo "   looping for ${MINUTES} min — output is on DeckLink sub-device 1,"
  echo "   which enumerates as \"DeckLink Quad (2)\": the SECOND BNC."
  END=$(( $(date +%s) + MINUTES * 60 ))
  while [ "$(date +%s)" -lt "$END" ]; do verse; done
else
  verse
fi

say "7. off"
cue take_out >/dev/null || true
sleep 2
echo "   log: $RUN/node.log"
grep -ciE "warn|error" "$RUN/node.log" | sed 's/^/   warnings+errors: /'
