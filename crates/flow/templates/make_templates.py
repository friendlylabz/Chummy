"""Writes the built-in template loop files that are generated rather than
written by hand in templates.rs. Run it after changing them:

    python3 crates/flow/templates/make_templates.py

- assistant-medium.chummy.json: the Chummy assistant (medium), the default
  loop, from assistant.py.
- assistant-small.chummy.json: the Chummy assistant (small), from small.py.
- assistant-large.chummy.json: the Chummy assistant (large), from large.py.

After changing one, add the old file's fingerprint (its SHA-256) to RETIRED
in crates/agent/src/library.rs, so unedited copies are upgraded.

Shared helpers (blocks, links, notes, functions, tidy) are in loopkit.py.
"""

import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the templates
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from loopkit import tidy  # noqa: E402
from assistant import CHUMMY, assistant  # noqa: E402
from small import SMALL  # noqa: E402
from large import LARGE  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

# Keys Chummy stores as decimal numbers. Writing them as Chummy does (200.0,
# not 200) makes each file exactly what Chummy would save, so the SHA-256
# of the file that crates/flow/build.rs works out is the loop's fingerprint,
# the one on Chummy's list of trusted loops.
FLOATS = {"x", "y", "width", "height", "temperature"}


def canonical(v, sort=False):
    """Block params are free-form JSON to Chummy, which keeps their keys
    sorted; everything else keeps the order of Chummy's own fields."""
    if isinstance(v, dict):
        items = sorted(v.items()) if sort else v.items()
        return {
            k: float(x) if k in FLOATS and isinstance(x, (int, float)) and not isinstance(x, bool)
            else canonical(x, sort or k == "params")
            for k, x in items
        }
    if isinstance(v, list):
        return [canonical(x, sort) for x in v]
    return v


for name, t in (("small", SMALL), ("medium", CHUMMY), ("large", LARGE)):
    with open(os.path.join(HERE, f"assistant-{name}.chummy.json"), "w", encoding="utf-8") as f:
        f.write(json.dumps(canonical(tidy(assistant(t))), indent=2, ensure_ascii=False) + "\n")
print("wrote the Chummy assistants (small, medium and large)")
