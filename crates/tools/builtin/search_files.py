# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "search_files"
# title = "Search files"
# version = "2.0.0"
# timeout = 60
# ///
"""Searches the text in the agent's workspace, like grep."""

import fnmatch
import os
import re

import chummy

SKIP = {".git", "node_modules", "__pycache__", ".venv", "venv", ".mypy_cache", ".pytest_cache", "dist", "build", "target"}
MAX_FILE = 2_000_000
MAX_CHARS = 30_000


def _files(root, file_glob):
    if os.path.isfile(root):
        yield root
        return
    for folder, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP and not d.startswith("."))
        for name in sorted(files):
            full = os.path.join(folder, name)
            rel = os.path.relpath(full, chummy.WORKSPACE)
            if file_glob and not (fnmatch.fnmatch(name, file_glob) or fnmatch.fnmatch(rel, file_glob)):
                continue
            yield full


@chummy.tool
def search_files(
    pattern: str,
    path: str = ".",
    regex: bool = False,
    file_glob: str = "",
    ignore_case: bool = True,
    context_lines: int = 0,
    max_results: int = 200,
) -> str:
    """Find the lines in the workspace's text files that contain a pattern, with file names and line numbers (like grep).

    pattern: the text to look for (or a regular expression, with regex)
    path: a folder or file to search, relative to the workspace; . is everything
    regex: treat pattern as a Python regular expression
    file_glob: only search files whose name matches, like *.py or docs/*.md
    ignore_case: match upper and lower case alike
    context_lines: also show this many lines before and after each match (0 to 5)
    max_results: most matching lines to show
    """
    if not pattern:
        raise chummy.ToolError("Give a pattern to search for.")
    flags = re.IGNORECASE if ignore_case else 0
    try:
        rx = re.compile(pattern if regex else re.escape(pattern), flags)
    except re.error as e:
        raise chummy.ToolError(f"The regular expression isn't valid: {e}")
    root = chummy.workspace_path(path)
    if not os.path.exists(root):
        raise chummy.ToolError(f"Not found: {path}")
    ctx = min(max(0, context_lines), 5)
    limit = max(1, max_results)
    out, hits, size, files_hit = [], 0, 0, 0
    for full in _files(root, file_glob):
        try:
            if os.path.getsize(full) > MAX_FILE:
                continue
            with open(full, encoding="utf-8") as f:
                lines = f.read().split("\n")
        except (UnicodeDecodeError, OSError):
            continue
        rel = os.path.relpath(full, chummy.WORKSPACE)
        matched = [i for i, l in enumerate(lines) if rx.search(l)]
        if not matched:
            continue
        files_hit += 1
        shown = set()
        for i in matched:
            if hits >= limit or size > MAX_CHARS:
                break
            hits += 1
            lo, hi = max(0, i - ctx), min(len(lines) - 1, i + ctx)
            if ctx and out and lo not in shown and (lo - 1) not in shown:
                out.append("--")
            for j in range(lo, hi + 1):
                if j in shown:
                    continue
                shown.add(j)
                sep = ":" if j == i or j in matched else "-"
                row = f"{rel}{sep}{j + 1}{sep} {lines[j].rstrip()[:300]}"
                out.append(row)
                size += len(row) + 1
        if hits >= limit or size > MAX_CHARS:
            out.append(f"[Stopped after {hits} matches. Narrow the search (path, file_glob) to see more.]")
            break
    if not out:
        return f"No lines match “{pattern}”."
    return "\n".join(out)
