# Chummy tools

A tool is **one Python file**. Chummy runs it in a WebAssembly sandbox: the
official CPython build for WASI, under Wasmtime. There is no way to run a
tool outside the sandbox.

Chummy ships with a set of built-in tools, installed when you set up tools in
Settings → Tools. Every tool is off until you switch it on, and the Tools
permission must be on too. Installing tools from files isn't offered in the
app yet; the installer below is what it will use.

## The file

```python
# /// script
# requires-python = ">=3.14"
# dependencies = ["beautifulsoup4==4.13.4"]
#
# [tool.chummy]
# name = "fetch_page"
# title = "Fetch web page"
# version = "1.0.0"
# network = true
# timeout = 60
# ///
"""Fetches a web page and returns its readable text."""

from bs4 import BeautifulSoup

import chummy


@chummy.tool
def fetch_page(url: str, max_chars: int = 6000) -> str:
    """Get the readable text of a web page.

    url: the full address, starting with https://
    max_chars: stop after this many characters
    """
    r = chummy.http.get(url).raise_for_status()
    ...
```

### Header

The header is a standard [PEP 723](https://peps.python.org/pep-0723/) inline
metadata block. Chummy reads it **without running the file**, so everything
you agree to is known before anything is installed.

| Field | Meaning |
|-------|---------|
| `requires-python` | Standard. Must allow the sandbox's Python (3.14). |
| `dependencies` | Standard. Plain PyPI requirements only: no URLs, paths or `pkg @ …` references. |
| `[tool.chummy] name` | The name the model calls: starts with `a`–`z`, then `a`–`z`, `0`–`9`, `_`; 48 characters at most. |
| `title` | Display name (defaults to the name). |
| `version` | Shown in Settings. |
| `network` | `true` if the tool needs the internet. It still can't go online until the user allows it. |
| `timeout` | Seconds a call may run (default 30, at most 300). |

Unknown keys in `[tool.chummy]` are errors. Other tools' tables are ignored.
The names of built-in tools are reserved.

### The function

Exactly one function is marked `@chummy.tool`. The model sees:

- **description**: the first paragraph of its docstring (keep it to one
  sentence for small models);
- **parameters**: from the signature. Types `str`, `int`, `float`, `bool`,
  `list[...]` and `Literal[...]` are supported; parameters with defaults are
  optional; `name: help` lines in the docstring become each parameter's help.

The function returns text (or anything JSON can hold). Raise
`chummy.ToolError("…")` with a short message when something's wrong ("File
not found: notes.md"); the model sees it as the result and can correct
itself. Other exceptions are reported as `TypeName: message`, without the
traceback. What the tool prints goes to its log (the first 64 KB are kept),
not to the model. Tool functions must be ordinary functions, not `async`,
and must return rather than call `sys.exit()`.

The model's arguments are checked against the parameters before the call:
numbers and true/false written as text are converted, `null` for an
optional parameter means its default, and anything else that doesn't fit is
explained to the model so it can try again.

### The `chummy` module

| Name | What it does |
|------|--------------|
| `@chummy.tool` | Marks the function the model calls. |
| `chummy.ToolError` | An error message for the model. |
| `chummy.workspace_path(p)` | Turns a path the model gave into one inside the workspace; refuses absolute paths and `..`. |
| `chummy.WORKSPACE` | `/workspace`, which is also the current folder. |
| `chummy.http.get(url, headers=None)`, `chummy.http.post(url, data=None, json=None, headers=None)` | Web requests made by the app on the tool's behalf. Return a `Response` with `status`, `url`, `headers`, `text`, `ok`, `json()`, `raise_for_status()`. |

## The sandbox

Each call gets a fresh WebAssembly instance that can see exactly:

| Path | What | Access |
|------|------|--------|
| `/workspace` | the agent's workspace folder | read/write |
| `/tool` | the tool file and its packages | read-only |
| `/lib` | Python's standard library | read-only |
| `/chummy` | Chummy's runner and helper module | read-only |

Nothing else on the computer exists for it. It gets no environment variables
from the computer (only `TZ`, the user's time zone). WASI preview 1 has no
sockets, so a tool has no network of its own: `chummy.http` sends the request
to the app, which makes it only if **all** of these hold:

1. the tool's header says `network = true`;
2. Internet access is on in Permissions;
3. the tool's own internet switch is on in Settings → Tools (turning it on
   asks for confirmation);
4. the address is `http`/`https` and doesn't point to this computer or the
   local network (loopback, private, link-local, carrier-grade NAT and
   similar ranges are refused, after resolving the name, and again at every
   redirect). The connection then goes to the addresses that were checked,
   so a second DNS answer can't point it somewhere else.

Responses are cut at 2 MB. Each call has a time limit (the header's
`timeout`) and a 512 MB memory limit.

## Installing

Installing a tool (built-in tools are installed this way):

1. Read and check the header (no code runs).
2. Check `requires-python` against the sandbox's Python.
3. Resolve the dependencies to a lock: a standard
   [PEP 751](https://peps.python.org/pep-0751/) `pylock.toml`. Built-in tools
   ship with theirs; for others, Chummy asks `uv` to resolve them for a
   WebAssembly platform, so only wheels without compiled code qualify.
4. Download each wheel, check its SHA-256 against the lock, and unpack it.
   Only pure-Python wheels (`…-none-any.whl`) are accepted, and files with
   compiled code are refused even in a mislabelled wheel. Nothing from a
   package runs while installing. A lock with environment markers, or for
   another Python version, is refused: locks for the sandbox are resolved
   for exactly one platform.
5. Load the tool once **inside the sandbox** to read its description and
   parameters.
6. Write `manifest.json`. The new version replaces the old one only when
   everything succeeded.

Installed tools live in the Chummy folder under `tools/installed/<name>/`
(`tool.py`, `pylock.toml`, `site-packages/`, `manifest.json`). Before every
call Chummy checks `tool.py` still matches the checksum recorded when it was
installed, and that it was installed for the current Python (after a Python
update, tools are reinstalled). Installs run one at a time.

## Built-in tools

| Tool | What it does | Internet |
|------|--------------|----------|
| `read_file` | Reads a text file in the workspace. | no |
| `write_file` | Writes or appends to a file in the workspace. | no |
| `list_files` | Lists a workspace folder. | no |
| `search_files` | Finds lines containing some text in the workspace's files. | no |
| `calculator` | Works out arithmetic exactly (no `eval`). | no |
| `current_time` | Today's date and the time, in any time zone (uses `tzdata`). | no |
| `fetch_page` | Gets the readable text of a web page (uses `beautifulsoup4`). | yes |
