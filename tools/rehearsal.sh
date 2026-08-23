#!/bin/bash
# The Canyon Hills service, stood up so a person can run it.
#
#   tools/rehearsal.sh              lower third on SDI, full screen + wall on NDI
#   tools/rehearsal.sh --all-ndi    no card needed, all three over NDI
#
# Builds the four looks, the Items with their several destinations, the two
# Follow playlists and the two Cue ones, and the rundown — then starts a node
# with three outputs and hands over. Open http://127.0.0.1:8519/show.
#
# This is a REHEARSAL, not a test: it is meant to find what the system cannot
# do. Nothing here asserts anything. Leave it running and drive it.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD
API=http://127.0.0.1:8519
ALL_NDI=0
# Build the service content, or keep what is already there.
#
# The run directory persists now, so a second run against an existing database
# would build a SECOND copy of the whole service beside the first —
# `rehearsal-service.py` posts, it does not reconcile. Keeping it is the safe
# default; --reseed is the deliberate act.
RESEED=0
# Sub-device 1: the second BNC on the Quad 2, and the one already known to be
# cabled to the converter.
DECKLINK=1
# The face the node renders with. Fonts travel as CAS assets in the real
# system; this is the stand-in until they do, and it is worth overriding
# because a look judged in the wrong typeface is not the look.
FONT="${PMG_FONT:-/System/Library/Fonts/Helvetica.ttc}"
while [ $# -gt 0 ]; do
  case "$1" in
    --all-ndi) ALL_NDI=1; shift ;;
    --reseed) RESEED=1; shift ;;
    --decklink) DECKLINK="$2"; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

# Under the repo's own gitignored var/, NOT under TMPDIR.
#
# macOS purges /var/folders/.../T/ on its own schedule, and it took a
# rehearsal plant with it — the database, the CAS, the node's identity, and a
# backup that had been put in a second temp directory, which is not a backup at
# all. A rehearsal is meant to be left running for days and driven; it needs to
# survive a Tuesday. Override with PMG_RUN to put it somewhere else.
RUN="${PMG_RUN:-$REPO/var/rehearsal}"
if [ "$RESEED" = 1 ]; then rm -rf "$RUN"; fi
mkdir -p "$RUN"
say() { printf "\n\033[1m== %s\033[0m\n" "$*"; }

say "0. no stragglers"
lsof -ti:8519 -sTCP:LISTEN | xargs -r kill 2>/dev/null || true
pkill -f "target/release/pmg-node" 2>/dev/null || true
sleep 2

say "1. build"
cargo build --release -p pmg-node -p pmg-cli 2>&1 | grep -E "^error" && exit 1

say "2. the web app"
# The server only serves the browser app when NODE_ENV=production, from dist/.
# Without this the API answers fine and every UI route is "Cannot GET /show" —
# which is what the first rehearsal ran into, having been told to open Show.
npm run build 2>&1 | tail -3

say "3. server"
NODE_ENV=production nohup env PMG_DB="$RUN/var/pmg.db" PMG_CAS="$RUN/var/cas" \
  node server/index.js > "$RUN/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 40); do curl -sf "$API/api/health" >/dev/null 2>&1 && break; sleep 0.5; done

say "4. the service"
if [ -s "$RUN/service.json" ] && [ "$RESEED" = 0 ]; then
  echo "Keeping the service already in $RUN."
  echo "Anything you built by hand is still there. --reseed to start over."
else
  # Not piped to /dev/null: if building the service fails, that IS the result of
  # the rehearsal and it must be on screen rather than swallowed.
  python3 tools/rehearsal-service.py "$API" "$RUN" | tee "$RUN/service.json"
fi

say "5. node — three outputs"
# One layer per output, named the same on each, because an Item's destinations
# name a layer per output and they have to exist to be armed into.
if [ "$ALL_NDI" = 1 ]; then
  # audio=2 on the feeds that carry clips, so the program monitor's meters
  # read something during a rehearsal instead of needing a flag nobody knows.
  OUTPUTS=(--output "id=lt,backend=ndi,name=CH Lower Third,layers=Graphics,audio=2"
           --output "id=fs,backend=ndi,name=CH Full Screen,layers=Graphics,audio=2"
           --output "id=wall,backend=ndi,name=CH LED Wall,layers=Graphics")
else
  OUTPUTS=(--output "id=lt,backend=decklink,device=$DECKLINK,mode=1080p59.94,layers=Graphics"
           --output "id=fs,backend=ndi,name=CH Full Screen,layers=Graphics"
           --output "id=wall,backend=ndi,name=CH LED Wall,layers=Graphics")
fi

rm -rf "$RUN/node"
nohup "$REPO/target/release/pmg-node" --state-dir "$RUN/node" --server ws://127.0.0.1:8519 \
  --name booth-1 "${OUTPUTS[@]}" \
  --font "$FONT" > "$RUN/node.log" 2>&1 &
NODE_PID=$!

for _ in $(seq 1 60); do
  NID=$(curl -s "$API/api/nodes" | python3 -c 'import sys,json;n=json.load(sys.stdin)["nodes"];print(n[0]["node_id"] if n else "")' 2>/dev/null || true)
  [ -n "$NID" ] && break
  sleep 1
done
[ -n "$NID" ] || { echo "node never linked:"; tail -20 "$RUN/node.log"; exit 1; }
curl -sf -X POST "$API/api/nodes/$NID/adopt" -H 'Content-Type: application/json' \
  -d '{"adopted":true}' >/dev/null

say "6. ready"
cat <<EOF

  Show view    $API/show
  Node log     $RUN/node.log
  Server log   $RUN/server.log

  Outputs
$( [ "$ALL_NDI" = 1 ] && echo "    lt    NDI  \"CH Lower Third\"" || echo "    lt    SDI  DeckLink sub-device $DECKLINK" )
    fs    NDI  "CH Full Screen"
    wall  NDI  "CH LED Wall"

  The service, in order:
$(python3 -c "
import json
d=json.load(open('$RUN/service.json'))
for n,m,c in d['playlists']: print(f'    {n:20} {m:7} {c:3} items')
" 2>/dev/null)

  Things worth trying, because they are the ones nobody has tried:
    · Arrow through the song at the speed you would follow a band.
    · Get back to the first Chorus slide the way you would if the leader
      called it — and time yourself. Sections survive only as a prefix in
      the label today, which is the point of trying it.
    · Take "Call to worship verse". The full screen is on_request, so it
      should NOT fire. Now try to fire it.
    · Take "Giving — full screen". The wall destination is deliberately
      blank; check the wall actually clears rather than keeping the last
      graphic.
    · In Sermon, take "Verse — part 1", then take part 2 and part 3.

  Both run DETACHED — this script exits and leaves them up. Stop them with:

    tools/rehearsal-stop.sh

  There is deliberately no trap: an EXIT trap killed the server every time
  this was run from anything that reaps its shell, and the symptom was a node
  logging "link down — still playing" while the browser showed nothing.
EOF
