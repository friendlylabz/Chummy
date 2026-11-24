# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "find_files"
# title = "Find files"
# version = "1.0.0"
# timeout = 60
# ///
"""Finds files in the agent's workspace by name pattern."""

import os
import pathlib

import chummy

SKIP = {".git", "node_modules", "__pycache__", ".venv", "venv"}
LIMIT = 1000


@chummy.tool
def find_files(pattern: str, path: str = ".") -> str:
    """Find files by name pattern, like *.py, report*.md or src/**/*.ts. Newest first.

    pattern: a name pattern; * matches anything in a name, ** any folders. Without a /, it looks in every folder.
    path: the folder to look in, relative to the workspace
    """
    if not pattern.strip():
        raise chummy.ToolError("Give a pattern, like *.py.")
    p = pattern.strip().replace("\\", "/")
    if p.startswith("/") or ".." in p.split("/"):
        raise chummy.ToolError("Use a pattern inside the workspace, like docs/*.md.")
    root = pathlib.Path(chummy.workspace_path(path))
    if not root.is_dir():
        raise chummy.ToolError(f"Folder not found: {path}")
    if "/" not in p and not p.startswith("**"):
        p = "**/" + p
    found = []
    for f in root.glob(p):
        rel = f.relative_to(chummy.WORKSPACE)
        if any(part in SKIP for part in rel.parts) or not f.is_file():
            continue
        try:
            found.append((f.stat().st_mtime, str(rel)))
        except OSError:
            continue
    if not found:
        return f"No files match {pattern}."
    found.sort(reverse=True)
    out = [name for _, name in found[:LIMIT]]
    if len(found) > LIMIT:
        out.append(f"[… and {len(found) - LIMIT} more. Use a narrower pattern.]")
    return "\n".join(out)
