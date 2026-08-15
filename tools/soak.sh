#!/bin/bash
# M3's soak: video looping on one layer, lower thirds firing on another.
#
#   tools/soak.sh --minutes 5          a quick check that it all still works
#   tools/soak.sh --hours 2            the one the roadmap asks for
#   tools/soak.sh --hours 2 --backend ndi
#
# NEEDS HARDWARE for the default DeckLink backend (sub-device 1 free). Use
# --backend ndi to run it without a card.
#
# The point is the VERDICT at the end, computed rather than eyeballed:
#
#   late frames          zero, or it is not broadcast equipment
#   held / dropped       the clip's decoder keeping up with the output
#   audio buffer trend   flat. A trend is lip sync walking, and it is the one
#                        fault with no error anywhere — see INTEGRATION-NOTES
#   audio peak           not silence, because a flat buffer full of zeroes
#                        graphs identically to a flat buffer full of programme
#
# Everything lands in a run directory it prints at the start, so a failed
# overnight run can be read in the morning rather than re-run.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD
API=http://127.0.0.1:8519

MINUTES=5
BACKEND=decklink
CYCLE=8          # seconds between lower-third take and take-out
while [ $# -gt 0 ]; do
  case "$1" in
    --minutes) MINUTES="$2"; shift 2 ;;
    --hours)   MINUTES=$(( $2 * 60 )); shift 2 ;;
    --backend) BACKEND="$2"; shift 2 ;;
    --cycle)   CYCLE="$2"; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

RUN="${TMPDIR:-/tmp}/pmg-soak-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN"
say() { printf "\n\033[1m== %s\033[0m\n" "$*" | tee -a "$RUN/soak.log"; }
note() { printf "   %s\n" "$*" | tee -a "$RUN/soak.log"; }

api() {
  local out code
  out=$(curl -s -w '\n%{http_code}' "$@")
  code=$(printf '%s' "$out" | tail -1)
  out=$(printf '%s' "$out" | sed '$d')
  if [ "$code" -ge 400 ]; then printf '   HTTP %s: %s\n' "$code" "$out" >&2; return 1; fi
  printf '%s' "$out"
}

say "soak: ${MINUTES} min, backend ${BACKEND}, lower third every ${CYCLE}s"
note "run directory $RUN"

say "0. no stragglers"
lsof -ti:8519 -sTCP:LISTEN | xargs -r kill 2>/dev/null || true
pkill -f "target/release/pmg-node" 2>/dev/null || true
sleep 2
if lsof -ti:8519 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "   something still holds 8519; refusing to measure it" >&2; exit 1
fi

say "1. build"
cargo build --release -p pmg-node -p pmg-cli 2>&1 | grep -E "^error" && exit 1
note "ok"

say "2. media"
CLIP="$RUN/../pmg-soak-reel.mov"
if [ ! -f "$CLIP" ]; then
  note "rendering a 30 s clip with a tone (once; reused by later runs)"
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=1920x1080:rate=60:duration=30" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=30" \
    -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le -c:a pcm_s16le -ac 2 "$CLIP"
fi
note "$(ls -lh "$CLIP" | awk '{print $5}') $CLIP"

say "3. server"
rm -rf "$RUN/var"
PMG_DB="$RUN/var/pmg.db" PMG_CAS="$RUN/var/cas" node server/index.js > "$RUN/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; kill ${NODE_PID:-0} 2>/dev/null || true' EXIT
for _ in $(seq 1 40); do
  curl -sf "$API/api/health" >/dev/null 2>&1 && break
  kill -0 $SERVER_PID 2>/dev/null || { echo "   server died:"; tail -5 "$RUN/server.log"; exit 1; }
  sleep 0.5
done
note "up"

say "4. ingest"
JOB=$(api -X POST "$API/api/assets/ingest?filename=reel.mov" \
  -H 'Content-Type: application/octet-stream' --data-binary @"$CLIP" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
for _ in $(seq 1 300); do
  J=$(api "$API/api/ingest/$JOB")
  ST=$(echo "$J" | python3 -c 'import sys,json;print(json.load(sys.stdin)["state"])')
  case "$ST" in done|failed|rejected) break ;; esac
  sleep 1
done
HASH=$(echo "$J" | python3 -c 'import sys,json;print(json.load(sys.stdin)["hash"])')
note "$ST · $HASH"

say "5. two templates: a clip and a lower third"
python3 tools/e2e-build-template.py "$HASH" "$RUN" >/dev/null
mv "$RUN/e2e-template.json" "$RUN/clip.json"
"$REPO/target/release/pmgctl" scene fixture "$RUN/third.json" >/dev/null
python3 - "$RUN/third.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
# A distinct id, or the second publish is refused as belonging to the first
# template — the manifest carries the document's own id.
d["id"] = "01JSOAKLOWERTHIRD0000001"
d["name"] = "Soak lower third"
for f in d.get("fonts", []):
    f.pop("asset", None)
json.dump(d, open(p, "w"), indent=2)
PY

publish() {  # <json> <label> -> template_rev on stdout
  local src=$1 label=$2
  "$REPO/target/release/pmgctl" scene compile "$src" "$RUN/$label.pmgscene" >/dev/null
  local tid
  tid=$(python3 -c "import json;print(json.load(open('$RUN/$label.manifest.json'))['template_id'])")
  api -X POST "$API/api/templates" -H 'Content-Type: application/json' \
    -d "{\"id\":\"$tid\",\"name\":\"$label\"}" >/dev/null
  python3 - "$RUN/$label.pmgscene" "$RUN/$label.manifest.json" <<'PY' > "$RUN/$label.publish.json"
import base64, json, sys
print(json.dumps({"scene": base64.b64encode(open(sys.argv[1],'rb').read()).decode(),
                  "manifest": json.load(open(sys.argv[2]))}))
PY
  api -X POST "$API/api/templates/$tid/publish" -H 'Content-Type: application/json' \
    --data-binary @"$RUN/$label.publish.json" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["template_rev"])'
}
CLIP_REV=$(publish "$RUN/clip.json" clip)
THIRD_REV=$(publish "$RUN/third.json" third)
note "clip  $CLIP_REV"
note "third $THIRD_REV"

item() {  # <rev> <label> <layer> -> item id
  api -X POST "$API/api/items" -H 'Content-Type: application/json' -d "$(python3 - <<PY
import json
print(json.dumps({"label": "$2", "template_rev": "$1",
                  "data": {"name": "Soak", "title": "$2"},
                  "destination": {"output": "out-0", "layer": "$3"}}))
PY
)" | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])'
}
CLIP_ITEM=$(item "$CLIP_REV" "Reel" Fullscreen)
THIRD_ITEM=$(item "$THIRD_REV" "Lower third" "Lower Third")

say "6. node"
rm -rf "$RUN/node"
NODE_ARGS=(--state-dir "$RUN/node" --server ws://127.0.0.1:8519 --name soak
           --layers "Fullscreen,Lower Third,Bug" --audio-channels 2
           --font /System/Library/Fonts/Helvetica.ttc)
if [ "$BACKEND" = "decklink" ]; then
  NODE_ARGS+=(--backend decklink --decklink 1 --decklink-mode "1080p59.94")
else
  NODE_ARGS+=(--backend ndi --ndi-name "PMG Soak")
fi
"$REPO/target/release/pmg-node" "${NODE_ARGS[@]}" > "$RUN/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 60); do
  NID=$(api "$API/api/nodes" | python3 -c 'import sys,json;n=json.load(sys.stdin)["nodes"];print(n[0]["node_id"] if n else "")' || true)
  [ -n "$NID" ] && break
  sleep 1
done
[ -n "$NID" ] || { echo "   node never linked:"; tail -15 "$RUN/node.log"; exit 1; }
api -X POST "$API/api/nodes/$NID/adopt" -H 'Content-Type: application/json' -d '{"adopted":true}' >/dev/null
note "$NID adopted"

cue() {  # <op> <item> <layer>
  api -X POST "$API/api/cue" -H 'Content-Type: application/json' -d "$(python3 - <<PY
import json, uuid
print(json.dumps({"node": "$NID", "cue_id": str(uuid.uuid4()), "op": "$1",
                  "item": "$2", "output": "out-0", "layer": "$3",
                  "deadline_ms": 2000}))
PY
)" >/dev/null 2>&1
}

say "7. clip on air"
for attempt in $(seq 1 60); do
  if cue take "$CLIP_ITEM" Fullscreen; then note "took on attempt $attempt"; break; fi
  sleep 2
done

say "8. soaking for ${MINUTES} min"
note "sampling every 30 s into $RUN/samples.tsv"
printf "t_s\tlate\tskipped\n" > "$RUN/samples.tsv"
END=$(( $(date +%s) + MINUTES * 60 ))
T0=$(date +%s)
NEXT_SAMPLE=$T0
CYCLES=0
LAST_LATE=0
while [ "$(date +%s)" -lt "$END" ]; do
  cue take "$THIRD_ITEM" "Lower Third" || true
  sleep "$CYCLE"
  cue take_out "$THIRD_ITEM" "Lower Third" || true
  sleep "$CYCLE"
  CYCLES=$(( CYCLES + 1 ))

  NOW=$(date +%s)
  if [ "$NOW" -ge "$NEXT_SAMPLE" ]; then
    NEXT_SAMPLE=$(( NOW + 30 ))
    LATE=$(api "$API/api/nodes/$NID" | python3 -c '
import sys, json
d = json.load(sys.stdin)
outs = (d.get("state") or {}).get("outputs", [])
print(sum(o.get("late_frames", 0) for o in outs))
' 2>/dev/null || echo "?")
    LAST_LATE=$LATE
    printf "%s\t%s\t-\n" "$(( NOW - T0 ))" "$LATE" >> "$RUN/samples.tsv"
    printf "\r   %4d min in · %d cycles · late %s " "$(( (NOW - T0) / 60 ))" "$CYCLES" "$LATE"
  fi
done
echo

say "9. verdict"
cue take_out "$THIRD_ITEM" "Lower Third" || true
sleep 2
python3 - "$RUN" "$CYCLES" "$LAST_LATE" <<'PYEOF' | tee -a "$RUN/soak.log"
import re, sys, statistics as st

run, cycles, api_late = sys.argv[1], int(sys.argv[2]), sys.argv[3]
# Strip ANSI defensively. The node writes plain text to a file now, but a run
# captured some other way still might not, and a verdict that silently finds
# nothing is worse than no verdict at all.
log = re.sub(r"\x1b\[[0-9;]*m", "", open(f"{run}/node.log", errors="replace").read())

def nums(pattern):
    return [float(m) for m in re.findall(pattern, log)]

bufs  = nums(r"audio_buffered=(\d+)")
peaks = nums(r'peak_dbfs="?(-?[\d.]+)"?')  # tracing quotes String fields
cardl = nums(r"late=(\d+)")
held  = nums(r"held=(\d+)")
drop  = nums(r"dropped=(\d+)")

fails, missing = 0, 0

def check(ok, label, detail):
    global fails
    fails += not ok
    print(f"  {'PASS' if ok else 'FAIL'}  {label:<22} {detail}")

def need(samples, label):
    """A metric with no samples is a FAILURE, not a skip.

    The first version of this printed PASS having matched nothing at all,
    because ANSI codes sat between every field name and its `=`. A harness that
    cannot tell "measured zero" from "measured nothing" is not an instrument.
    """
    global missing
    if not samples:
        missing += 1
        print(f"  ????  {label:<22} NO SAMPLES - the log never contained this metric")
        return False
    return True

# Two different late counters, and they mean different things: one is our own
# frame budget, the other is what the card says a viewer actually saw.
print(f"  ----  render over budget    {api_late}  (our budget, from the node's own report)")

if need(cardl, "card displayed late"):
    check(max(cardl) == 0, "card displayed late", f"{max(cardl):.0f}  (what a viewer would see)")

if need(held, "clip held frames"):
    check(max(held) == 0, "clip held frames",
          f"{max(held):.0f}" + ("" if max(held) == 0 else "  (decoder fell behind)"))
    print(f"  ----  clip dropped frames   {max(drop) if drop else 0:.0f}  (expected when media runs faster)")

if need(bufs, "audio buffer trend"):
    # First tenth against last tenth rather than a line fit: the level
    # oscillates within each frame, and what matters is whether the floor moved.
    n = max(1, len(bufs) // 10)
    first, last = st.mean(bufs[:n]), st.mean(bufs[-n:])
    ms = (last - first) * 1000 / 48000
    check(abs(ms) < 20, "audio buffer trend",
          f"{first:.0f} -> {last:.0f} samples ({ms:+.1f} ms over the run)")

if need(peaks, "audio peak"):
    loud = max(peaks)
    check(loud > -60, "audio peak",
          f"{loud:.1f} dBFS  ({'programme' if loud > -60 else 'SILENCE'})")

print(f"\n  {cycles} lower-third take/take-out cycles over the run")
print(f"  full log: {run}/node.log")

if missing:
    print(f"\nSOAK INCONCLUSIVE - {missing} metric(s) had no samples. Nothing was proved.")
    sys.exit(2)
print("\n" + ("SOAK PASSED" if fails == 0 else f"SOAK FAILED - {fails} check(s)"))
sys.exit(1 if fails else 0)
PYEOF
