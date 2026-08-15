"""Build a template that plays the ingested clip full-frame under a lower third.

Starts from the reference fixture so every field the format requires is present
and correct, rather than hand-writing a document and discovering the schema one
error at a time.
"""
import json
import subprocess
import sys

REPO = "/Users/jbeale/Code/prodmesh-graphics"
asset_hash = sys.argv[1]

subprocess.run(
    [f"{REPO}/target/release/pmgctl", "scene", "fixture", "e2e-base.json"],
    check=True, capture_output=True,
)
d = json.load(open("e2e-base.json"))
d["name"] = "Clip with sound"
# The manifest carries the document's own id and the server checks it against
# the template being published to, so the two must be decided together.
d["id"] = "01JE2ECLIPWITHSOUND00001"

# The fixture's font points at an all-zero placeholder hash. Leaving it in means
# the node tries to stage an asset that does not exist and the Item is blocked —
# correctly, but for a reason that has nothing to do with what is under test.
# The node runs with --font instead.
for f in d.get("fonts", []):
    f.pop("asset", None)

# The asset table is what lets a node find the bytes: the document names a
# local key, the assignment carries the hash, and the node stages it.
d["assets"] = [{"key": "reel", "hash": asset_hash, "kind": "video"}]

d["root"]["children"].insert(0, {
    "id": "reel",
    "display_name": "Reel",
    "kind": "video",
    "layout": {
        "position": "absolute",
        "left": {"Px": 0.0}, "top": {"Px": 0.0},
        "width": {"Px": 1920.0}, "height": {"Px": 1080.0},
    },
    "content": {"kind": "video", "asset_key": "reel", "fit": "cover", "loop": True},
    "children": [],
})

json.dump(d, open("e2e-template.json", "w"), indent=2)
print("wrote e2e-template.json")
