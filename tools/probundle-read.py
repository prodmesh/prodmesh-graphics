"""Read the lyrics and section structure out of a ProPresenter .probundle.

    python3 tools/probundle-read.py <file.probundle>   -> JSON on stdout

**A reader, never a writer, and deliberately not a full parser.** A `.pro` file
is protobuf whose schema is Renewed Vision's, not ours; decoding it properly
would mean vendoring their `.proto` and tracking it across their releases. What
this needs is narrower — the words on each slide and the section each belongs
to — and both are recoverable from the byte stream without a schema:

  * slide text is an RTF blob, brace-balanced, so it can be found and closed
    exactly rather than guessed at with a regex;
  * groups are `\n&\n$<uuid>\x12<len><name>`, followed by the ids of the cues
    they contain — so a section name and its membership are both recoverable;
  * each slide is preceded by its cue id, so scanning back from an RTF blob to
    the nearest id that some group claimed gives that slide's section.

The arrangement holds more slots than there are slides (31 against 19 here)
because a chorus is *referenced* by each section that sings it rather than
copied. That is the same fact ADR 0018 is about, arrived at from the other
direction, and it is why sections are read as membership rather than by
partitioning an ordered list.

That is enough to run a rehearsal on real content, and it stops at the point
where being wrong would be silent. Anything it cannot read it says it cannot
read.

The bundles are also **not quite valid zips** — the central directory is
reported 98 bytes long and Python's zipfile refuses them where `unzip` merely
complains. So the archive is read with `unzip`, which compensates.
"""

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

UUID = rb"\$([0-9A-F\-]{36})"
# A group: its own id, then its display name.
GROUP = re.compile(rb"\n&\n" + UUID + rb"\x12(.)")

# Names that are a song section rather than a playlist or a theme. The file
# also defines groups called "Worship" and "Default Collection", which are
# collections the document belongs to and not part of its structure.
SECTIONS = re.compile(
    rb"(Intro|Verse(?: \d+)?|Pre-?Chorus(?: \d+)?|Chorus(?: \d+)?|Bridge(?: \d+)?"
    rb"|Tag(?: \d+)?|Vamp(?: \d+)?|Interlude(?: \d+)?|Instrumental(?: \d+)?"
    rb"|Ending|Outro|Turnaround|Refrain(?: \d+)?|Blank|Misc(?: \d+)?)$"
)


def rtf_blobs(b):
    """Every RTF blob with its offset, closed by brace balance."""
    out = []
    i = 0
    while True:
        i = b.find(b"{\\rtf1", i)
        if i < 0:
            return out
        depth, j = 0, i
        while j < len(b):
            c = b[j]
            if c == 0x5C:          # backslash escapes the next byte
                j += 2
                continue
            if c == 0x7B:
                depth += 1
            elif c == 0x7D:
                depth -= 1
                if depth == 0:
                    out.append((i, b[i:j + 1]))
                    break
            j += 1
        i = j + 1


def strip_group(s, opener):
    """Drop an RTF group by brace balance, wherever it starts."""
    while True:
        i = s.find(opener)
        if i < 0:
            return s
        depth, j = 0, i
        while j < len(s):
            if s[j] == "\\":
                j += 2
                continue
            if s[j] == "{":
                depth += 1
            elif s[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        s = s[:i] + s[j + 1:]


def rtf_text(blob):
    """The words, without the typesetting."""
    s = blob.decode("latin-1")
    # Balanced removal, not a regex: these groups nest, and a regex that gets
    # it nearly right leaks "HelveticaNeue-Bold;" into the lyrics — which is
    # wrong in the quiet way, because it still looks like a slide.
    for opener in ("{\\fonttbl", "{\\colortbl", "{\\*\\expandedcolortbl"):
        s = strip_group(s, opener)
    # cp1252, not latin-1: \'92 is a curly apostrophe, and "he\u0092s not dead"
    # would go on a screen in front of a room.
    s = re.sub(r"\\'([0-9a-fA-F]{2})",
               lambda m: bytes([int(m.group(1), 16)]).decode("cp1252", "replace"), s)
    s = s.replace("\\\n", "\n")            # RTF line break
    s = re.sub(r"\\par\b", "\n", s)
    s = re.sub(r"\\[a-zA-Z]+-?\d*\s?", "", s)
    s = s.replace("{", "").replace("}", "")
    # Collapse the runs of blank lines RTF leaves behind, but keep real ones:
    # a lyric slide's line breaks are the slide's shape.
    lines = [ln.strip() for ln in s.split("\n")]
    return "\n".join(ln for ln in lines if ln).strip()


def groups(b, first_slide):
    """cue id -> the section names that claim it, in document order.

    A group's cue ids are the uuids between its name and the next group, which
    holds because every group is defined before any slide body.
    """
    found = []
    for m in GROUP.finditer(b):
        n = b[m.end() - 1]
        name = b[m.end():m.end() + n]
        if SECTIONS.match(name):
            found.append((m.start(), name.decode(), m.end() + n))

    claims, order = {}, []
    for i, (_, name, end) in enumerate(found):
        stop = found[i + 1][0] if i + 1 < len(found) else first_slide
        order.append(name)
        for cue in re.findall(UUID, b[end:stop]):
            claims.setdefault(cue, []).append(name)
    return claims, order


def read(path):
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["unzip", "-qo", str(path), "-d", tmp],
                       capture_output=True)     # complains about the 98 bytes; fine
        pro = sorted(Path(tmp).rglob("*.pro"), key=lambda p: p.stat().st_size)
        if not pro:
            raise SystemExit(f"{path}: no .pro document inside")
        # The largest .pro is the presentation; anything else is a companion.
        doc = pro[-1]
        doc.chmod(0o644)
        b = doc.read_bytes()

    blobs = [(o, t) for o, t in ((o, rtf_text(x)) for o, x in rtf_blobs(b)) if t]
    if not blobs:
        raise SystemExit(f"{path}: no slide text found")

    claims, order = groups(b, blobs[0][0])
    ids = [(m.start(), m.group(1)) for m in re.finditer(UUID, b)]

    slides, unmapped = [], 0
    for off, text in blobs:
        # Back to the nearest id some group claimed. Nearest rather than a
        # fixed distance: the number of uuids between a cue id and its slide
        # body varies with what else the slide carries.
        owner = next((u for o, u in reversed(ids) if o < off and u in claims), None)
        if owner is None:
            unmapped += 1
        slides.append({"sections": claims.get(owner, []), "text": text})

    # In singing order.
    #
    # Slides are stored in whatever order the document grew in — the first one
    # in this file belongs to the Ending — so the arrangement is what makes it
    # a song rather than a bag of slides. A section that appears twice gets its
    # slides twice, which is exactly what the operator arrows through.
    running = [
        {"section": name, "text": sl["text"]}
        for name in order
        for sl in slides if name in sl["sections"]
    ]
    # A document with no sections is not broken — a sermon is a flat list of
    # slides and only songs have verses. Its running order is the order the
    # slides are stored in, and `arrangement` stays empty to say so.
    if not order:
        running = [{"section": None, "text": sl["text"]} for sl in slides]

    return {
        "title": Path(path).stem,
        "document": doc.name,
        "running_order": running,
        # In singing order, and longer than `slides` when a chorus repeats:
        # sections REFERENCE slides rather than owning copies of them.
        "arrangement": order,
        # Set when some slide could not be attributed. Reported rather than
        # papered over — a rehearsal that silently loses a section would send
        # us looking for a UI bug that was a parsing bug.
        "unmapped_slides": unmapped,
        "slides": slides,
    }


if __name__ == "__main__":
    print(json.dumps(read(sys.argv[1]), indent=2))
