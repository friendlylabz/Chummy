# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "read_file"
# title = "Read file"
# version = "1.0.0"
# ///
"""Reads a text file from the agent's workspace."""

import chummy


@chummy.tool
def read_file(path: str, max_chars: int = 8000) -> str:
    """Read a text file in the workspace.

    path: the file, relative to the workspace, like notes/todo.md
    max_chars: stop after this many characters
    """
    full = chummy.workspace_path(path)
    try:
        with open(full, encoding="utf-8", errors="replace") as f:
            text = f.read(max(1, max_chars) + 1)
    except FileNotFoundError:
        raise chummy.ToolError(f"File not found: {path}")
    except IsADirectoryError:
        raise chummy.ToolError(f"{path} is a folder. Use list_files to see what's inside.")
    if len(text) > max_chars:
        return text[:max_chars] + f"\n[… cut at {max_chars} characters]"
    return text
