"""Writes the built-in template loop files that are generated rather than
written by hand in templates.rs. Run it after changing them:

    python3 crates/flow/templates/make_templates.py

- assistant.chummy.json: the Chummy assistant (the default loop), from
  assistant.py.
- terminal.chummy.json: the Terminal assistant (the same loop, working only
  through the terminal), from terminal.py.

Shared helpers (blocks, links, notes, functions, tidy) are in loopkit.py.
"""

import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the templates
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from loopkit import tidy  # noqa: E402
from assistant import assistant  # noqa: E402
from terminal import terminal  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

for name, make in (("assistant", assistant), ("terminal", terminal)):
    with open(os.path.join(HERE, f"{name}.chummy.json"), "w", encoding="utf-8") as f:
        f.write(json.dumps(tidy(make()), indent=2, ensure_ascii=False) + "\n")
print("wrote the Chummy assistant and the Terminal assistant")
