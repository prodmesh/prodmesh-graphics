"""The four looks the Canyon Hills rundown asks for, from one set of fields.

    python3 tools/rehearsal-templates.py <outdir>

Every look takes the same two fields — `name` and `title` — which is the point:
one Item's data feeds the lower third, the full-screen version, and the LED
wall, each rendering it differently (ADR 0018). If these needed different fields
the operator would be typing the verse three times, which is the thing the
rundown says is painful today.

Built by deriving from `pmgctl scene fixture` rather than hand-writing four
documents, so every field the format requires is present and correct instead of
being discovered one validation error at a time.
"""

import copy
import json
import subprocess
import sys

REPO = "/Users/jbeale/Code/prodmesh-graphics"
OUT = sys.argv[1]

subprocess.run(
    [f"{REPO}/target/release/pmgctl", "scene", "fixture", f"{OUT}/base.json"],
    check=True, capture_output=True,
)
BASE = json.load(open(f"{OUT}/base.json"))


def find(node, node_id):
    if node["id"] == node_id:
        return node
    for c in node.get("children", []):
        got = find(c, node_id)
        if got:
            return got
    return None


def strip_ellipsize(node):
    """Remove the 40-character truncation the fixture ships with.

    It exists to keep the sample tidy. Left in, a long verse and a short one
    truncate to the same width and the box never moves — which would flatten
    the one thing at 29:30 the rundown marks as impossible today.
    """
    b = (node.get("content") or {}).get("binding") or {}
    if "transform" in b:
        b["transform"] = [
            t for t in b["transform"]
            if not (isinstance(t, dict) and "ellipsize" in t)
        ]
    for c in node.get("children", []):
        strip_ellipsize(c)


def restyle(root, node_id, size, colour=None, align="start", max_lines=1):
    """Set a text node's type.

    Size and alignment live on `content.font`/`content.align`; the colour is the
    node's own `style.fill`. Worth writing down once: they are three different
    places and guessing costs a validation error each time.
    """
    n = find(root, node_id)
    if not n:
        return
    n["content"]["font"]["size"] = size
    n["content"]["align"] = align
    # A verse is several lines. Left at 1 the solver has nothing to grow into
    # and the box cannot track the text, which is the whole demonstration.
    n["content"]["max_lines"] = max_lines
    if colour:
        n["style"]["fill"] = {"rgba": colour}


def text_field(d, key, label, max_len):
    """Resize a declared field so it can hold what this look is for.

    The fixture declares `name` as a NAME: `max_len: 40`, which is right for
    "Pastor Dan Whitfield" and wrong for four lines of scripture. The server
    checks an Item's data against every destination's template at save, so
    leaving it would refuse every real lyric — correctly, and for a reason that
    is about the fixture rather than about the system.

    Worth knowing: the check is across ALL destinations, so the narrowest one
    binds. A lower third that caps at 160 and a wall that allows 400 give an
    Item 160, which is the right answer and not an obvious one.
    """
    for f in d.get("fields", []):
        if f["key"] == key:
            f["label"] = label
            if max_len is None:
                f.pop("max_len", None)
            else:
                f["max_len"] = max_len


def look(doc_id, name, build):
    d = copy.deepcopy(BASE)
    d["id"] = doc_id
    d["name"] = name
    # The fixture's font points at an all-zero placeholder hash. Left in, the
    # node tries to stage an asset that does not exist and every Item is
    # blocked — correctly, and for a reason that has nothing to do with the
    # rehearsal. The node runs with --font instead.
    for f in d.get("fonts", []):
        f.pop("asset", None)
    strip_ellipsize(d["root"])
    build(d)
    path = f"{OUT}/{doc_id}.json"
    json.dump(d, open(path, "w"), indent=2)
    return path


# --- 1. The lower third -----------------------------------------------------
# The fixture already is one. Nothing to do but keep it honest.
def lower_third(d):
    # Up to three lines. At 29:30 the verse grows and shrinks against this
    # box — with one line it can only ever get wider, which is half the effect.
    restyle(d["root"], "name", 46.0, max_lines=3)
    # The narrowest of the three, and therefore the one that decides how long a
    # line an operator can type for an Item that also goes to the wall.
    text_field(d, "name", "Line", 160)
    text_field(d, "title", "Reference", None)


# --- 2. The full-screen version ---------------------------------------------
def full_screen(d):
    """The same words, centred, over a full-frame plate.

    The rundown: *"Lower thirds always have a full screen version we can take
    if we need to."* Same Item, same data, a different shape.
    """
    root = d["root"]
    root["layout"]["justify"] = "center"
    root["layout"]["align"] = "center"
    root["layout"]["padding"] = {"top": 0.0, "right": 160.0, "bottom": 0.0, "left": 160.0}
    # A full-frame plate rather than a band. A full-screen graphic is not keyed
    # over camera — it replaces the picture — so it needs its own ground.
    root["style"]["fill"] = {"rgba": [0.02, 0.03, 0.05, 1.0]}

    band = find(root, "band")
    band["layout"]["align"] = "center"
    band["layout"]["max_width"] = {"Px": 1500.0}
    band["layout"]["padding"] = {"top": 40.0, "right": 60.0, "bottom": 40.0, "left": 60.0}
    band["style"]["fill"] = {"rgba": [0.0, 0.0, 0.0, 0.0]}
    band["transform"]["anchor"] = [0.5, 0.5]

    restyle(root, "name", 84.0, align="center", max_lines=4)
    restyle(root, "title", 40.0, align="center")
    text_field(d, "name", "Line", 400)
    text_field(d, "title", "Reference", None)


# --- 3. The LED wall --------------------------------------------------------
def led_wall(d):
    """*"LED Wall also has its own look with the lyric."*

    Bigger and higher-contrast than either: it is read from across a room, at
    an angle, competing with house lights. Deliberately not a scaled copy of
    the full-screen look — that is why it is a third template and not a
    resolution setting.
    """
    root = d["root"]
    root["layout"]["justify"] = "center"
    root["layout"]["align"] = "center"
    root["layout"]["padding"] = {"top": 0.0, "right": 90.0, "bottom": 0.0, "left": 90.0}
    root["style"]["fill"] = {"rgba": [0.0, 0.0, 0.0, 1.0]}

    band = find(root, "band")
    band["layout"]["align"] = "center"
    band["layout"]["max_width"] = {"Px": 1740.0}
    band["layout"]["padding"] = {"top": 30.0, "right": 40.0, "bottom": 30.0, "left": 40.0}
    band["style"]["fill"] = {"rgba": [0.0, 0.0, 0.0, 0.0]}
    band["transform"]["anchor"] = [0.5, 0.5]

    restyle(root, "name", 104.0, [1.0, 1.0, 1.0, 1.0], "center", max_lines=4)
    restyle(root, "title", 46.0, [0.72, 0.80, 0.92, 1.0], "center")
    text_field(d, "name", "Line", 400)
    text_field(d, "title", "Reference", None)


# --- 4. The name tag --------------------------------------------------------
def name_tag(d):
    """Worship leader and pastor: a narrower band that never needs to grow.

    Kept distinct from the verse look even though they are close, because the
    rundown treats them as different objects and an operator picking "name tag"
    should not get a template sized for four lines of scripture.
    """
    band = find(d["root"], "band")
    band["layout"]["max_width"] = {"Px": 900.0}
    # A name really is short, so this one keeps the cap. It is also never a
    # destination of the same Item as a verse look.
    text_field(d, "name", "Name", 40)


LOOKS = [
    ("01JCHREHEARSALLOWER3RD01", "Lower third", lower_third),
    ("01JCHREHEARSALFULLSCRN01", "Full screen", full_screen),
    ("01JCHREHEARSALLEDWALL001", "LED wall", led_wall),
    ("01JCHREHEARSALNAMETAG001", "Name tag", name_tag),
]

print(json.dumps([
    {"id": i, "name": n, "path": look(i, n, b)} for i, n, b in LOOKS
]))
