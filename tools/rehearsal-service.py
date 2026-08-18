"""Build the Canyon Hills service on a running server.

    python3 tools/rehearsal-service.py <api-base> <workdir>

The rundown in `docs/reference/canyon-hills-service-rundown.md`, constructed as
Templates, Items, Playlists and a Rundown so it can be *run* rather than read.
Kept as data at the top of this file, so what the rehearsal exercises can be
checked against the reference document line by line.

The point is not the content. It is that this shape — three destinations on
nearly every Item, two playout modes in one rundown, a full-screen version that
waits to be asked for, a wall that goes deliberately blank — has never been
driven by a person. Whatever it turns out this cannot do is the output.
"""

import base64
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

API, WORK = sys.argv[1], sys.argv[2]
# Real service content, if it is there. The invented lyrics below are a
# fallback so the rehearsal runs on any machine — but a song the operator
# actually knows is the difference between testing Follow mode and testing
# whether they can read.
BUNDLES = os.path.expanduser("~/Downloads")
REPO = "/Users/jbeale/Code/prodmesh-graphics"
PMGCTL = f"{REPO}/target/release/pmgctl"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"{API}{path}", data=data, method=method,
        headers={"content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as r:
            return json.loads(r.read() or "{}")
    except urllib.error.HTTPError as e:
        # The server's own sentence. Swallowing it is how a harness ends up
        # lying about what it built — the failure mode this repo has hit more
        # than once.
        raise SystemExit(f"{method} {path} -> {e.code}: {e.read().decode()}")


# ---------------------------------------------------------------------------
# Outputs, named as the rundown names them.
# ---------------------------------------------------------------------------
LT, FS, WALL = "lt", "fs", "wall"
LAYER = "Graphics"


def imported(name):
    """Slides out of a .probundle, or None if it is not there."""
    path = os.path.join(BUNDLES, f"{name}.probundle")
    if not os.path.exists(path):
        return None
    try:
        d = json.loads(subprocess.run(
            ["python3", f"{REPO}/tools/probundle-read.py", path],
            check=True, capture_output=True, text=True).stdout)
    except Exception as e:                      # noqa: BLE001 — a rehearsal aid
        print(f"  (could not read {name}: {e})", file=sys.stderr)
        return None
    # Slide labels and stray single characters are other text boxes on the
    # slide, not the words. Anything under four characters is not a lyric.
    d["running_order"] = [s for s in d["running_order"] if len(s["text"]) > 3]
    return d


def dest(output, rev, mode="auto", blank=False):
    return {"output": output, "layer": LAYER, "template_rev": rev,
            "mode": mode, "blank": blank}


# ---------------------------------------------------------------------------
# The service. Each entry is one row of the reference rundown.
# ---------------------------------------------------------------------------
def build_service(rev):
    """rev: {'lower3rd'|'fullscrn'|'ledwall'|'nametag'} -> template_rev"""
    lt, fs, wall = rev["lower3rd"], rev["fullscrn"], rev["ledwall"]
    tag, scr = rev["nametag"], rev["scripture"]

    # 0:00 and 27:25 — name tags. Lower third ONLY. A name tag has no
    # full-screen version and the wall does not carry it.
    call_to_worship = [
        ("Worship leader", {"name": "Grace Okafor", "title": "Worship Leader"},
         [dest(LT, tag)]),
        # 0:30 — "+ Full Screen 1080p version of the verse". The full screen is
        # there to be taken IF NEEDED, which is `on_request`: it must not fire
        # with the lower third.
        ("Call to worship verse",
         {"name": "Let everything that has breath praise the Lord.", "title": "Psalm 150:6"},
         [dest(LT, scr), dest(FS, fs, mode="on_request"), dest(WALL, wall)]),
    ]

    # 2:10 — the worship set. Follow mode: navigating IS taking.
    #
    # Sections are a property of the playlist ENTRY, which is where the
    # arrangement lives: the same chorus slide is sung in Chorus 1 and again in
    # Chorus 2. Read straight out of the .probundle.
    def lyric(section, line, song):
        # The section travels as a playlist ENTRY property now, not as a label
        # prefix — the label goes back to being the words on the slide.
        return (line[:44], {"name": line, "title": song},
                [dest(LT, lt), dest(FS, fs), dest(WALL, wall)], section)

    song1 = [
        lyric("Verse 1", "My hope is built on nothing less", "Cornerstone"),
        lyric("Verse 1", "Than Jesus' blood and righteousness", "Cornerstone"),
        lyric("Chorus", "Christ alone, cornerstone", "Cornerstone"),
        lyric("Chorus", "Weak made strong in the Saviour's love", "Cornerstone"),
        lyric("Verse 2", "When darkness seems to hide His face", "Cornerstone"),
        lyric("Verse 2", "I rest on His unchanging grace", "Cornerstone"),
        # 10:41 — "Worship leader suddenly decides to sing the Chorus again."
        # The operator has to get back to the first Chorus slide under time
        # pressure. Today that means finding it in a list.
    ]

    song2 = [
        lyric("Verse 1", "Amazing grace, how sweet the sound", "Amazing Grace"),
        lyric("Verse 1", "That saved a wretch like me", "Amazing Grace"),
        lyric("Chorus", "My chains are gone, I've been set free", "Amazing Grace"),
        lyric("Verse 2", "'Twas grace that taught my heart to fear", "Amazing Grace"),
    ]

    # 28:00 onward — the sermon. Cue mode: arm, then take.
    sermon = [
        ("Sermon title", {"name": "The Weight of Glory", "title": "Romans 8:18-25"},
         [dest(LT, lt), dest(FS, fs, mode="on_request"), dest(WALL, wall)]),
        # 29:00 and 29:30 — the verse, in sections. Taking the next section is
        # an Update on the live Item, which is the tween. Both are here as
        # separate Items so the operator can also take them as cuts and see the
        # difference.
        ("Verse — part 1",
         {"name": "For I consider that the sufferings of this present time",
          "title": "Romans 8:18"},
         [dest(LT, scr), dest(FS, fs), dest(WALL, wall)]),
        ("Verse — part 2",
         {"name": "are not worth comparing with the glory that is to be revealed to us.",
          "title": "Romans 8:18"},
         [dest(LT, scr), dest(FS, fs), dest(WALL, wall)]),
        ("Verse — part 3",
         {"name": "For the creation waits with eager longing.", "title": "Romans 8:19"},
         [dest(LT, scr), dest(FS, fs), dest(WALL, wall)]),
        ("Pastor", {"name": "Dan Whitfield", "title": "Lead Pastor"}, [dest(LT, tag)]),
        # 30:00+ — "Full screen graphics typically blank out the LED wall
        # leaving only a background." A blank destination is an INSTRUCTION,
        # not an absence, and this is the row that proves it travels as one.
        ("Giving — full screen",
         {"name": "Give at canyonhills.church/give", "title": "Thank you"},
         [dest(FS, fs), dest(WALL, wall, blank=True)]),
        ("Prayer after the service",
         {"name": "Prayer is available at the front", "title": "After the service"},
         [dest(LT, lt), dest(FS, fs), dest(WALL, wall)]),
    ]

    playlists = [("Call to Worship", "cue", call_to_worship)]

    # The real song, if it is on this machine. Its sections are the ones the
    # worship leader calls out, which is what makes "jump to the Chorus"
    # a real test rather than a demonstration of scrolling.
    song = imported("SONG King Above It All")
    if song:
        playlists.append((song["title"].removeprefix("SONG ").strip(), "follow", [
            lyric(sl["section"] or "—", sl["text"].replace("\n", " "), song["title"])
            for sl in song["running_order"]
        ]))
    else:
        playlists.append(("Cornerstone", "follow", song1))
    playlists.append(("Amazing Grace", "follow", song2))

    # The real sermon deck goes AFTER the scripted sermon rows, so the three
    # things the rundown calls out by name — the title, the verse in parts, the
    # blanked wall — are still exercised in order.
    message = imported("Message")
    if message:
        sermon += [
            (sl["text"].replace("\n", " ")[:44],
             {"name": sl["text"].replace("\n", " "), "title": "Psalm 51"},
             [dest(LT, scr), dest(FS, fs), dest(WALL, wall)])
            for sl in message["running_order"][:12]
        ]

    playlists.append(("Sermon", "cue", sermon))
    return playlists


# ---------------------------------------------------------------------------

def publish(doc_id, name, path):
    """Compile and publish one template, returning its revision."""
    scene = f"{WORK}/{doc_id}.pmgscene"
    subprocess.run([PMGCTL, "scene", "compile", path, scene],
                   check=True, capture_output=True)
    manifest = json.load(open(f"{WORK}/{doc_id}.manifest.json"))
    call("POST", "/api/templates", {"id": doc_id, "name": name})
    r = call("POST", f"/api/templates/{doc_id}/publish", {
        "scene": base64.b64encode(open(scene, "rb").read()).decode(),
        "manifest": manifest,
    })
    return r["template_rev"]


def main():
    looks = json.loads(subprocess.run(
        ["python3", f"{REPO}/tools/rehearsal-templates.py", WORK],
        check=True, capture_output=True, text=True,
    ).stdout)

    key = {"01JCHREHEARSALLOWER3RD01": "lower3rd", "01JCHREHEARSALFULLSCRN01": "fullscrn",
           "01JCHREHEARSALLEDWALL001": "ledwall", "01JCHREHEARSALNAMETAG001": "nametag",
           "01JCHREHEARSALSCRIPTUR01": "scripture"}
    rev = {key[l["id"]]: publish(l["id"], l["name"], l["path"]) for l in looks}

    playlist_ids = []
    counts = []
    for name, mode, items in build_service(rev):
        pl = call("POST", "/api/playlists", {"name": name, "mode": mode})
        ids = []
        for entry in items:
            label, data, dests = entry[0], entry[1], entry[2]
            section = entry[3] if len(entry) > 3 else None
            it = call("POST", "/api/items", {
                "label": label, "data": data,
                "template_rev": dests[0]["template_rev"],
                "destinations": dests,
            })
            ids.append({"item": it["id"], "section": section})
        call("PUT", f"/api/playlists/{pl['id']}/items", {"items": ids})
        playlist_ids.append(pl["id"])
        counts.append((name, mode, len(ids)))

    rd = call("POST", "/api/rundowns", {"name": "Sunday 09:00"})
    call("PUT", f"/api/rundowns/{rd['id']}/playlists", {"playlists": playlist_ids})

    print(json.dumps({"rundown": rd["id"], "playlists": counts,
                      "templates": rev}, indent=2))


main()
