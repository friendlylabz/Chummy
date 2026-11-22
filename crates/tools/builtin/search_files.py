# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "search_files"
# title = "Search files"
# version = "1.0.0"
# timeout = 60
# ///
"""Searches the text files in the agent's workspace."""

import os

import chummy

SKIP = {".git", "node_modules", "__pycache__", ".venv"}


@chummy.tool
def search_files(text: str, path: str = ".", max_results: int = 20) -> str:
    """Find lines containing some text in the workspace's files. Ignores case.

    text: what to look for
    path: the folder to search, relative to the workspace
    max_results: most matching lines to return
    """
    needle = text.lower().strip()
    if not needle:
        raise chummy.ToolError("Give some text to search for.")
    root = chummy.workspace_path(path)
    hits = []
    for folder, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP)
        for name in sorted(files):
            full = os.path.join(folder, name)
            rel = os.path.relpath(full, chummy.WORKSPACE)
            try:
                if os.path.getsize(full) > 2_000_000:
                    continue
                with open(full, encoding="utf-8") as f:
                    for n, line in enumerate(f, 1):
                        if needle in line.lower():
                            hits.append(f"{rel}:{n}: {line.strip()[:200]}")
                            if len(hits) >= max_results:
                                return "\n".join(hits) + "\n[… more results not shown]"
            except (UnicodeDecodeError, OSError):
                continue
    return "\n".join(hits) if hits else f"No lines contain “{text}”."
