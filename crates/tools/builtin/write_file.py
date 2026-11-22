# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "write_file"
# title = "Write file"
# version = "1.0.0"
# ///
"""Writes a text file in the agent's workspace."""

import os

import chummy


@chummy.tool
def write_file(path: str, content: str, append: bool = False) -> str:
    """Write text to a file in the workspace, creating folders as needed.

    path: the file, relative to the workspace, like notes/todo.md
    content: the text to write
    append: add to the end of the file instead of replacing it
    """
    full = chummy.workspace_path(path)
    if full == chummy.WORKSPACE:
        raise chummy.ToolError("Give a file name, like notes.md.")
    os.makedirs(os.path.dirname(full), exist_ok=True)
    if os.path.isdir(full):
        raise chummy.ToolError(f"{path} is a folder.")
    with open(full, "a" if append else "w", encoding="utf-8") as f:
        f.write(content)
    return f"{'Added' if append else 'Wrote'} {len(content)} characters to {path}."
