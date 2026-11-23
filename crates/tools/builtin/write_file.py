# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "write_file"
# title = "Write file"
# version = "2.0.0"
# ///
"""Writes a text file in the agent's workspace."""

import os

import chummy


def _lines(text):
    return text.count("\n") + (1 if text and not text.endswith("\n") else 0)


@chummy.tool
def write_file(path: str, content: str, append: bool = False) -> str:
    """Create a text file, or replace all of one, in the workspace (folders are made as needed). To change part of a file, use edit_file instead.

    path: the file, relative to the workspace, like notes/todo.md
    content: the whole text of the file
    append: add the text to the end of the file instead of replacing it
    """
    full = chummy.workspace_path(path)
    if full == chummy.WORKSPACE:
        raise chummy.ToolError("Give a file name, like notes.md.")
    if os.path.isdir(full):
        raise chummy.ToolError(f"{path} is a folder.")
    os.makedirs(os.path.dirname(full), exist_ok=True)
    existed = os.path.exists(full)
    before = 0
    if existed:
        with open(full, encoding="utf-8", errors="replace") as f:
            before = _lines(f.read())
    with open(full, "a" if append else "w", encoding="utf-8") as f:
        f.write(content)
    n = _lines(content)
    if append and existed:
        return f"Added {n} line{'s' if n != 1 else ''} to {path} (now about {before + n})."
    if existed:
        return f"Replaced {path}: {n} line{'s' if n != 1 else ''} (it had {before})."
    return f"Created {path}: {n} line{'s' if n != 1 else ''}."
