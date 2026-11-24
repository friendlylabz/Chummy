# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "read_file"
# title = "Read file"
# version = "2.0.0"
# ///
"""Reads a text file from the agent's workspace, a chunk of lines at a time."""

import chummy

#: The most text one call returns, so a result fits a small model's context.
MAX_CHARS = 40_000
#: Longer lines are shortened.
MAX_LINE = 2_000


def _n(x):
    return f"{x:,}"


@chummy.tool
def read_file(path: str, line_number: int = 1, max_lines: int = 1000) -> str:
    """Read a text file in the workspace, with line numbers, a chunk of lines at a time.

    path: the file, relative to the workspace, like notes/todo.md
    line_number: the first line to read (1 is the start). To read on, use the number the result gives.
    max_lines: how many lines to read, 1 to 5000
    """
    full = chummy.workspace_path(path)
    try:
        with open(full, "rb") as f:
            head = f.read(8192)
        if b"\0" in head:
            raise chummy.ToolError(f"{path} looks like a binary file, not text.")
        with open(full, encoding="utf-8", errors="replace", newline=None) as f:
            lines = f.read().split("\n")
    except FileNotFoundError:
        raise chummy.ToolError(f"File not found: {path}")
    except IsADirectoryError:
        raise chummy.ToolError(f"{path} is a folder. Use list_files to see what's inside.")
    if lines and lines[-1] == "":
        lines.pop()
    total = len(lines)
    if total == 0:
        return f"{path} is empty."
    start = max(1, line_number)
    if start > total:
        raise chummy.ToolError(f"{path} has only {_n(total)} lines.")
    count = min(max(1, max_lines), 5000)
    out, size, last = [], 0, start - 1
    for n in range(start, min(total, start + count - 1) + 1):
        text = lines[n - 1]
        if len(text) > MAX_LINE:
            text = text[:MAX_LINE] + " […line cut]"
        row = f"{n:>6}\t{text}"
        if out and size + len(row) + 1 > MAX_CHARS:
            break
        out.append(row)
        size += len(row) + 1
        last = n
    head = f"{path}: lines {_n(start)}–{_n(last)} of {_n(total)}"
    if last < total:
        tail = f"[Lines {_n(last + 1)}–{_n(total)} not shown. To read on, use line_number={last + 1}.]"
        return head + "\n" + "\n".join(out) + "\n" + tail
    return head + "\n" + "\n".join(out)
