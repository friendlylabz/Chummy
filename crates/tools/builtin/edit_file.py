# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "edit_file"
# title = "Edit file"
# version = "1.0.0"
# ///
"""Changes part of a text file in the agent's workspace by exact replacement.

The model copies the text it wants to change (old_text) and says what goes
there instead (new_text), the way coding agents edit files: no line numbers
to get wrong, no patch format to get wrong, and an edit that doesn't match
the file exactly is refused with a hint rather than applied in the wrong
place. Small slips are forgiven when the result is still unambiguous:
read_file's line numbers pasted into old_text, and spaces at line ends.
"""

import difflib
import re

import chummy

NUMBERED = re.compile(r"^ *\d+\t")


def _strip_numbers(text):
    """Removes read_file's "   12<tab>" prefixes if every line has one."""
    lines = text.split("\n")
    if lines and all(NUMBERED.match(l) for l in lines if l.strip()):
        return "\n".join(NUMBERED.sub("", l, count=1) for l in lines)
    return None


def _line_of(text, index):
    return text.count("\n", 0, index) + 1


def _loose_spans(text, old):
    """Where old matches text line by line, ignoring spaces at line ends."""
    want = [l.rstrip() for l in old.split("\n")]
    while want and want[-1] == "":
        want.pop()
    if not want:
        return []
    lines = text.split("\n")
    starts, pos = [], 0
    for l in lines:
        starts.append(pos)
        pos += len(l) + 1
    spans = []
    for i in range(len(lines) - len(want) + 1):
        if all(lines[i + j].rstrip() == want[j] for j in range(len(want))):
            end = starts[i + len(want) - 1] + len(lines[i + len(want) - 1])
            spans.append((starts[i], end))
    return spans


def _closest(text, old):
    first = next((l.strip() for l in old.split("\n") if l.strip()), "")
    lines = text.split("\n")
    best = difflib.get_close_matches(first, [l.strip() for l in lines], n=1, cutoff=0.5)
    if not best:
        return ""
    n = next(i for i, l in enumerate(lines, 1) if l.strip() == best[0])
    return f" The closest line is {n}: {lines[n - 1].strip()[:200]}"


def _snippet(text, first, last):
    lines = text.split("\n")
    lo, hi = max(1, first - 2), min(len(lines), last + 2)
    if hi - lo > 30:
        hi = lo + 30
    return "\n".join(f"{n:>6}\t{lines[n - 1]}" for n in range(lo, hi + 1))


@chummy.tool
def edit_file(path: str, old_text: str, new_text: str, replace_all: bool = False) -> str:
    """Change part of a text file: replace old_text with new_text. old_text must match the file exactly, and only once unless replace_all is true. Read the file first.

    path: the file, relative to the workspace, like notes/todo.md
    old_text: the exact text to change, copied from the file (without read_file's line numbers); include a line or two around the change so it matches only one place
    new_text: the text to put there instead; empty deletes old_text
    replace_all: change every place old_text appears, not just one
    """
    full = chummy.workspace_path(path)
    try:
        with open(full, "rb") as f:
            raw = f.read()
    except FileNotFoundError:
        raise chummy.ToolError(f"File not found: {path}. To make a new file, use write_file.")
    except IsADirectoryError:
        raise chummy.ToolError(f"{path} is a folder.")
    if b"\0" in raw[:8192]:
        raise chummy.ToolError(f"{path} looks like a binary file, not text.")
    crlf = b"\r\n" in raw
    text = raw.decode("utf-8", errors="replace").replace("\r\n", "\n")
    old = old_text.replace("\r\n", "\n")
    new = new_text.replace("\r\n", "\n")
    if old == "":
        raise chummy.ToolError("old_text is empty. Copy the text to change from the file; to write a whole file, use write_file.")
    if old == new:
        raise chummy.ToolError("old_text and new_text are the same, so there's nothing to change.")

    spans = [(m.start(), m.end()) for m in re.finditer(re.escape(old), text)]
    if not spans:
        # Line numbers copied from read_file?
        bare = _strip_numbers(old)
        if bare is not None and bare in text:
            old = bare
            new = _strip_numbers(new) if _strip_numbers(new) is not None else new
            spans = [(m.start(), m.end()) for m in re.finditer(re.escape(old), text)]
    if not spans:
        spans = _loose_spans(text, old)
    if not spans:
        raise chummy.ToolError(
            f"old_text doesn't match {path} exactly (check spaces, indentation and line breaks)."
            + _closest(text, old)
            + " Read the file again and copy the text exactly."
        )
    if len(spans) > 1 and not replace_all:
        where = ", ".join(str(_line_of(text, s)) for s, _ in spans[:10])
        raise chummy.ToolError(
            f"old_text matches {len(spans)} places in {path} (lines {where}). Include more lines around the "
            "change so it matches one place, or set replace_all to change them all."
        )

    out, pos = [], 0
    for s, e in spans:
        out.append(text[pos:s])
        out.append(new)
        pos = e
    out.append(text[pos:])
    result = "".join(out)
    with open(full, "w", encoding="utf-8", newline="") as f:
        f.write(result.replace("\n", "\r\n") if crlf else result)

    first_start = spans[0][0]
    first = _line_of(result, first_start)
    last = first + max(new.count("\n"), 0)
    what = f"{len(spans)} places" if len(spans) > 1 else "1 place"
    return f"Edited {path}: changed {what}. Lines {first}–{last} now read:\n{_snippet(result, first, last)}"
