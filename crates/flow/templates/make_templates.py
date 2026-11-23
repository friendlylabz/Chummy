"""Writes the built-in template loop files that are generated rather than
written by hand in templates.rs. Run it after changing them:

    python3 crates/flow/templates/make_templates.py

- assistant.chummy.json: the Chummy assistant (the default loop), from
  assistant.py.

Shared helpers (blocks, links, notes, functions, tidy) are in loopkit.py.
"""

import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the templates
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from loopkit import tidy  # noqa: E402
from assistant import assistant  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

with open(os.path.join(HERE, "assistant.chummy.json"), "w", encoding="utf-8") as f:
    f.write(json.dumps(tidy(assistant()), indent=2, ensure_ascii=False) + "\n")
print("wrote the Chummy assistant")
