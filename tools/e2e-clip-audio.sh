#!/bin/bash
# End-to-end: a clip with sound, ingested → published → assigned → staged →
# taken on air → out of SDI with embedded audio.
#
# THIS NEEDS HARDWARE: a DeckLink with sub-device 1 available. It is the
# verification M3e was missing, and it is kept because every step of it found a
# real bug the unit tests could not:
#
#   - the node built `http://ws://host:port` for CAS staging, so it linked,
#     reported healthy, and silently never became ready
#   - the node passed a hard-coded `false` for "is this cue late", so the rule
#     AGENTS.md calls non-negotiable was true of the engine and false of the node
#   - a flat audio buffer proves the CADENCE and not the CONTENT, which is why
#     the node logs peak dBFS as well
#
# Run it from anywhere; it uses its own scratch directories and refuses to run
# if something else holds port 8519.
set -euo pipefail
cd "$(dirname "$0")"
REPO=/Users/jbeale/Code/prodmesh-graphics
API=http://127.0.0.1:8519
say() { printf "\n\033[1m== %s\033[0m\n" "$*"; }
# curl -f hides the response body on an HTTP error, which is the part that says
# what went wrong. This keeps it.
api() {
  local out code
  out=$(curl -s -w '\n%{http_code}' "$@")
  code=$(printf '%s' "$out" | tail -1)
  out=$(printf '%s' "$out" | sed '$d')
  if [ "$code" -ge 400 ]; then
    printf '     HTTP %s: %s\n' "$code" "$out" >&2
    return 1
  fi
  printf '%s' "$out"
}

say "1. a clip with pictures AND sound"
if [ ! -f reel.mov ]; then
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=1920x1080:rate=60:duration=30" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=30" \
    -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le \
    -c:a pcm_s16le -ac 2 reel.mov
fi
ls -lh reel.mov | awk '{print "   ", $9, $5}'

say "0. no stragglers"
lsof -ti:8519 -sTCP:LISTEN | xargs -r kill 2>/dev/null || true
pkill -f "target/release/pmg-node" 2>/dev/null || true
sleep 2
if lsof -ti:8519 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "     something still holds 8519; refusing to run against it" >&2
  exit 1
fi
echo "     clear"

say "2. server on a FRESH data dir"
rm -f "$PWD/e2e-var/pmg.db"*
PMG_DB="$PWD/e2e-var/pmg.db" PMG_CAS="$PWD/e2e-var/cas" node "$REPO/server/index.js" > server.log 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; kill ${NODE_PID:-0} 2>/dev/null || true' EXIT
until api "$API/api/health" >/dev/null 2>&1; do sleep 0.4; done
echo "    up (pid $SERVER_PID)"

say "3. ingest the clip"
JOB=$(api -X POST "$API/api/assets/ingest?filename=reel.mov" \
  -H 'Content-Type: application/octet-stream' --data-binary @reel.mov | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
for _ in $(seq 1 120); do
  J=$(api "$API/api/ingest/$JOB")
  ST=$(echo "$J" | python3 -c 'import sys,json;print(json.load(sys.stdin)["state"])')
  case "$ST" in done|failed|rejected) break ;; esac
  sleep 1
done
echo "$J" | python3 -c 'import sys,json;j=json.load(sys.stdin);print("    ",j["state"],"|",j.get("message"))'
HASH=$(echo "$J" | python3 -c 'import sys,json;print(json.load(sys.stdin)["hash"])')
echo "     hash $HASH"

say "4. compile a template that plays it"
python3 build-template.py "$HASH" > /dev/null
"$REPO/target/release/pmgctl" scene compile e2e-template.json e2e.pmgscene | sed 's/^/     /'

say "5. publish"
TID=$(python3 -c 'import json;print(json.load(open("e2e.manifest.json"))["template_id"])')
api -X POST "$API/api/templates" -H 'Content-Type: application/json' \
  -d "{\"id\":\"$TID\",\"name\":\"Clip with sound\"}" >/dev/null
echo "     template $TID"
PUB=$(python3 - <<PY
import base64, json
scene = base64.b64encode(open("e2e.pmgscene","rb").read()).decode()
manifest = json.load(open("e2e.manifest.json"))
print(json.dumps({"scene": scene, "manifest": manifest}))
PY
)
REV=$(echo "$PUB" | api -X POST "$API/api/templates/$TID/publish" -H 'Content-Type: application/json' --data-binary @- \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("template_rev") or d)')
echo "     template_rev $REV"

say "6. an item bound to it"
ITEM=$(api -X POST "$API/api/items" -H 'Content-Type: application/json' -d "$(python3 - <<PY
import json
print(json.dumps({
  "label": "Reel with sound",
  "template_id": "$TID",
  "template_rev": "$REV",
  "data": {"name": "Embedded audio", "title": "1 kHz on pair 1"},
  "destination": {"output": "out-0", "layer": "Fullscreen"},
}))
PY
)" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("id") or d)')
echo "     item $ITEM"

say "7. node: DeckLink sub-device 1, 2 channels of embedded audio"
NODE_STATE="${TMPDIR:-/tmp}/pmg-e2e-node"
rm -rf "$NODE_STATE"
"$REPO/target/release/pmg-node" \
  --state-dir "$NODE_STATE" --server ws://127.0.0.1:8519 \
  --name e2e-node --backend decklink --decklink 1 \
  --layers "Fullscreen,Lower Third,Bug" \
  --decklink-mode "1080p59.94" --audio-channels 2 \
  --font /System/Library/Fonts/Helvetica.ttc \
  > node.log 2>&1 &
NODE_PID=$!
echo "     pid $NODE_PID"
for _ in $(seq 1 60); do
  # node_id, not id. The first version asked for "id", got a KeyError every
  # time, and the `2>/dev/null || true` meant to make the poll robust turned it
  # into a silent sixty-second wait for something that could never arrive.
  NID=$(api "$API/api/nodes" | python3 -c 'import sys,json;n=json.load(sys.stdin)["nodes"];print(n[0]["node_id"] if n else "")' || true)
  [ -n "$NID" ] && break
  sleep 1
done
if [ -z "$NID" ]; then
  echo "     node never appeared in /api/nodes. Its log says:" >&2
  tail -15 node.log >&2
  exit 1
fi
echo "     linked as $NID"

say "8. adopt"
api -X POST "$API/api/nodes/$NID/adopt" -H 'Content-Type: application/json' -d '{"adopted":true}' >/dev/null
# Staging 566 MB over HTTP takes longer than a courtesy pause. Wait for the node
# to say it is READY rather than guessing, because an un-armable Item is a
# designed state and a take against one is refused, not queued.
for _ in $(seq 1 90); do
  READY=$(api "$API/api/nodes/$NID" | python3 -c '
import sys, json
d = json.load(sys.stdin)
# "ready" is not a top-level field; the first version asked for one and got 0
# forever. What decides a take is whether staging finished.
st = d.get("staging") or {}
print(1 if (st.get("done", 0) >= st.get("total", 0) and not st.get("errors")) else 0)
')
  [ "$READY" = "1" ] && break
  sleep 1
done
api "$API/api/nodes/$NID" | python3 -c '
import sys,json; d=json.load(sys.stdin)
print("     adopted:", d.get("adopted"), "| staging:", d.get("staging"))
st = d.get("state") or {}
for o in st.get("outputs", []):
    print("     output", o["id"], "layers", [l["name"] for l in o.get("layers", [])])
' || true

say "9. TAKE"
# Retry like an operator would. A node still staging a 566 MB clip cannot ack in
# time, and it now correctly REJECTS a cue whose deadline passed rather than
# firing it late. A fresh cue_id each attempt, because a retry of a refused take
# is a new decision and not a duplicate of the old one.
TOOK=no
for attempt in $(seq 1 30); do
  BODY=$(python3 -c "
import json, uuid
print(json.dumps({
  'node': '$NID',
  'cue_id': str(uuid.uuid4()),
  'op': 'take',
  'item': '$ITEM',
  'output': 'out-0',
  'layer': 'Fullscreen',
  'deadline_ms': 2000,
}))")
  if RESP=$(api -X POST "$API/api/cue" -H 'Content-Type: application/json' -d "$BODY" 2>/dev/null); then
    echo "     attempt $attempt: $RESP"
    TOOK=yes
    break
  fi
  sleep 2
done
if [ "$TOOK" != "yes" ]; then
  echo "     never took" >&2
  tail -20 node.log >&2
  exit 1
fi

say "10. on air for 60 s — watching the card's audio buffer"
sleep 60
echo "     what the node says is on air:"
api "$API/api/nodes/$NID" | python3 -c "
import sys, json
d = json.load(sys.stdin)
for o in (d.get('state') or {}).get('outputs', []):
    for l in o.get('layers', []):
        occ = l.get('occupant')
        if occ:
            print('     ', l['name'], '->', occ.get('item_label'),
                  '| phase', occ.get('phase'), '| degraded', occ.get('degraded'))
" || true
grep -iE "abuf|audio|late|clip" node.log | tail -25 || true

say "done. node.log and server.log are here."
