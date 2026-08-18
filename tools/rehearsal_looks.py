"""The looks Canyon Hills actually uses, from three reference frames.

Imported into `rehearsal-templates.py` — hence the underscore in the filename
while every other tool here uses a hyphen: this one is a module, not a script.

Kept separate because these are drawn
from screenshots of the real broadcast and are meant to be compared against
them — not derived from the reference fixture, which is a demonstration of the
format rather than a design anybody uses.

Three genuinely different graphics, all taking the same two fields:

    lyric      two lines, centred low, all caps, white, NO box
    nametag    bottom left, name over role, all caps, white, NO box
    scripture  a yellow reference chip above a teal body block, bottom left

The last one is the interesting one. It is two filled blocks, and the body
block is the thing that has to grow and shrink as the verse changes — which is
the 29:30 row of the rundown, drawn the way the room actually sees it.

**No box behind the lyric or the name tag.** Both sit over camera, keyed, and
the reference frames have no plate at all — just type with enough weight to
hold against a dark stage. That is a real constraint on the renderer rather
than a style choice: white type over live video with no plate is where a badly
premultiplied alpha edge shows up immediately.
"""

import copy

# Sampled from the reference frames.
WHITE = [1.0, 1.0, 1.0, 1.0]
YELLOW = [0.96, 0.91, 0.05, 1.0]        # the reference chip
TEAL = [0.18, 0.55, 0.60, 1.0]          # the body block
TEAL_DEEP = [0.08, 0.30, 0.36, 1.0]     # chip text, dark on yellow
CREAM = [0.98, 0.96, 0.78, 1.0]         # body text on teal


def box(node_id, name, **layout):
    """A plain container. Every field the format requires, none it does not."""
    return {
        "id": node_id,
        "display_name": name,
        "kind": "box",
        "layout": {
            "direction": "column", "justify": "start", "align": "start",
            "grow": 0.0, "shrink": 0.0, "width": "Content", "height": "Content",
            "margin": {"top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0},
            "padding": {"top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0},
            "gap": 0.0, "position": "relative", **layout,
        },
        "transform": {"translate": [0.0, 0.0], "rotate": 0.0, "scale": [1.0, 1.0],
                      "anchor": [0.0, 0.5], "skew": [0.0, 0.0]},
        "style": {"radius": 0.0, "opacity": 1.0},
        "children": [],
    }


def text(proto, node_id, name, field, *, size, colour, weight=700,
         align="start", max_lines=1, upper=True, line_height=1.05, tracking=0.0):
    """A text node, cloned from a known-good one so the schema stays right.

    `upper` uses the format's own binding transform rather than expecting an
    operator to type in capitals. The words in the data stay the words; the
    look decides how they are set. That is the whole point of a template, and
    it is also what lets one Item feed a shouty lyric slide and a sentence-case
    LED wall at the same time.
    """
    n = copy.deepcopy(proto)
    n["id"] = node_id
    n["display_name"] = name
    n["style"] = {"fill": {"rgba": colour}, "radius": 0.0, "opacity": 1.0}
    n["layout"] = {**n["layout"], "width": "Content", "height": "Content",
                   "grow": 0.0, "shrink": 0.0,
                   "margin": {"top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0},
                   "padding": {"top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0}}
    n["content"] = {
        "kind": "text",
        "binding": {"expr": "${%s}" % field,
                    "transform": ["trim", "upper"] if upper else ["trim"]},
        "font": {"family": "IBM Plex Sans", "weight": weight, "size": size,
                 "features": ["kern", "liga"], "tracking": tracking,
                 "line_height": line_height},
        "align": align,
        "max_lines": max_lines,
    }
    return n


def dissolve(d, node_id, in_ms=260.0, out_ms=260.0):
    """Replace the template's animation with a plain cross-dissolve.

    The reference fixture slides its band in from x = -900, which is a fine
    demonstration and wrong for lyrics: a worship set steps a slide every few
    seconds and an entrance animation on each one is motion nobody asked for.
    What a lyric slide wants is to be replaced, not to arrive.

    `In` and `Out` are ordinary named timelines — nothing about them is special
    to the engine — so this is just two opacity tracks. Taking over an occupied
    layer runs the old graphic's `Out` while the new one runs its `In`, which
    is what makes the two together read as a cross-fade.
    """
    fade = lambda a, b, ms: {                                    # noqa: E731
        "target": f"{node_id}/style.opacity",
        "blend": "replace",
        "keys": [
            {"t": 0.0, "v": a, "ease": {"cubic": [0.4, 0.0, 0.2, 1.0]}},
            {"t": ms, "v": b, "ease": "linear"},
        ],
    }
    d["timelines"] = [
        {"name": "In", "duration_ms": in_ms, "on_complete": "hold",
         "tracks": [fade(0.0, 1.0, in_ms)]},
        {"name": "Out", "duration_ms": out_ms, "on_complete": "evict",
         "tracks": [fade(1.0, 0.0, out_ms)]},
    ]


def root(width=1920.0, height=1080.0, **layout):
    # justify/align default to bottom-left and are overridable, so a caller
    # passing justify= does not collide with a hard-coded one.
    return box("root", "Root",
               width={"Px": width}, height={"Px": height},
               **{"justify": "end", "align": "start", **layout})


# ---------------------------------------------------------------------------

def lyric(d, proto):
    """Two lines, centred low, all caps, white, no plate.

    Reference frame: "WE ARE HERE TO WITNESS / TO SHARE AND TESTIFY".
    """
    r = root(justify="end", align="center")
    r["layout"]["padding"] = {"top": 0.0, "right": 120.0, "bottom": 70.0, "left": 120.0}
    body = box("band", "Lyric", align="center", max_width={"Px": 1560.0}, gap=0.0)
    body["transform"]["anchor"] = [0.5, 0.5]
    # Two lines is the shape of a lyric slide, and the reference frames never
    # show three. A third would push into the lower-third safe area.
    body["children"] = [text(proto, "name", "Lines", "name", size=62.0,
                             colour=WHITE, weight=800, align="center",
                             max_lines=2, line_height=1.18, tracking=0.01)]
    r["children"] = [body]
    d["root"] = r
    # Fades on the TEXT node rather than the band: the band has no fill in this
    # look, so fading it would animate nothing visible.
    dissolve(d, "name")


def nametag(d, proto):
    """Name over role, bottom left, all caps, white, no plate.

    Reference frame: "ANDY SOEMO / WORSHIP PASTOR".
    """
    r = root()
    r["layout"]["padding"] = {"top": 0.0, "right": 0.0, "bottom": 96.0, "left": 96.0}
    band = box("band", "Name tag", gap=4.0, max_width={"Px": 1100.0})
    band["children"] = [
        text(proto, "name", "Name", "name", size=68.0, colour=WHITE, weight=800,
             tracking=0.01),
        # Noticeably smaller and lighter than the name in the reference, which
        # is what makes it read as a role rather than a second name.
        text(proto, "title", "Role", "title", size=30.0, colour=WHITE, weight=600,
             tracking=0.06),
    ]
    r["children"] = [band]
    d["root"] = r


def scripture(d, proto):
    """A yellow reference chip above a teal body block.

    Reference frame: "I TIMOTHY 1:15" on yellow, over three lines of verse on
    teal. **This is the one that has to tween**: the teal block is sized by the
    words in it, so moving to the next section of a reading grows or shrinks
    it, which is the 29:30 row of the rundown.

    The chip is a box with its own fill wrapping a text node, because a text
    node's `style.fill` is the colour of the GLYPHS, not a background. Worth
    stating once — it is the kind of thing that looks like it should work and
    silently produces dark text on nothing.
    """
    r = root()
    r["layout"]["padding"] = {"top": 0.0, "right": 60.0, "bottom": 60.0, "left": 60.0}

    band = box("band", "Scripture", gap=0.0, align="start")

    chip = box("chip", "Reference chip", align="start")
    chip["layout"]["padding"] = {"top": 12.0, "right": 26.0, "bottom": 10.0, "left": 26.0}
    chip["style"]["fill"] = {"rgba": YELLOW}
    chip["children"] = [text(proto, "title", "Reference", "title", size=38.0,
                             colour=TEAL_DEEP, weight=800, tracking=0.05)]

    body = box("body", "Verse block", align="start", max_width={"Px": 1800.0})
    body["layout"]["padding"] = {"top": 22.0, "right": 40.0, "bottom": 26.0, "left": 26.0}
    body["style"]["fill"] = {"rgba": TEAL}
    # Clipped, so the tween's outgoing text is bounded by the block as it
    # morphs rather than spilling outside it while it shrinks.
    body["clip"] = True
    body["children"] = [text(proto, "name", "Verse", "name", size=44.0,
                             colour=CREAM, weight=600, max_lines=4,
                             upper=False, line_height=1.32)]

    band["children"] = [chip, body]
    r["children"] = [band]
    d["root"] = r
