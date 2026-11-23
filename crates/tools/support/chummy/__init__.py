"""Helpers for Chummy tools.

A Chummy tool is one Python file with one function marked ``@chummy.tool``.
It runs inside a sandbox (WebAssembly by default, or a container): it can see
the agent's workspace (as the current folder) and nothing else on the computer, and it can reach the
internet only through ``chummy.http``, and only if the user allowed it.

    import chummy

    @chummy.tool
    def word_count(path: str) -> str:
        \"\"\"Count the words in a text file.

        path: the file, relative to the workspace.
        \"\"\"
        with open(chummy.workspace_path(path), encoding="utf-8") as f:
            return str(len(f.read().split()))
"""

import json as _json
import os as _os
import posixpath as _pp
import sys as _sys

__all__ = ["tool", "ToolError", "workspace_path", "http", "Response", "WORKSPACE"]

#: Where the agent's workspace is mounted. It is also the current folder.
#: (Usually /workspace; a sandbox that copies the workspace in says where.)
WORKSPACE = (_os.environ.get("CHUMMY_WORKSPACE") or "/workspace").rstrip("/") or "/workspace"

_registered = []


class ToolError(Exception):
    """Raise this with a short, plain message for the model.

    The message is shown to the model as the tool's result, so it can
    correct itself ("File not found: notes.md").
    """


def tool(fn):
    """Marks the function the model calls. Use it on exactly one function."""
    _registered.append(fn)
    return fn


def workspace_path(path):
    """Turns a path the model gave into a path inside the workspace.

    Refuses absolute paths and ``..``, so a tool only ever touches the
    workspace. (The sandbox enforces this too; this gives a clearer error.)
    """
    if not isinstance(path, str):
        raise ToolError("The path must be text.")
    p = path.strip().replace("\\", "/")
    if p in ("", "."):
        return WORKSPACE
    if p.startswith("/") or (len(p) > 1 and p[1] == ":"):
        raise ToolError("Use a path inside the workspace, like notes/todo.md, not an absolute path.")
    parts = [x for x in p.split("/") if x not in ("", ".")]
    if ".." in parts:
        raise ToolError("Paths can't use .. to leave the workspace.")
    return _pp.join(WORKSPACE, *parts)


# ---------------------------------------------------------------- the bridge

_next_id = 0


def _request(op, **fields):
    """Asks the Chummy app to do something on the tool's behalf."""
    global _next_id
    _next_id += 1
    msg = {"op": op, "id": _next_id}
    msg.update(fields)
    out = _sys.__stdout__
    out.write(_json.dumps(msg) + "\n")
    out.flush()
    line = _sys.stdin.readline()
    if not line:
        raise ToolError("Chummy stopped answering.")
    reply = _json.loads(line)
    if not reply.get("ok"):
        raise ToolError(reply.get("error") or "The request failed.")
    return reply


class Response:
    """A web response: ``status``, ``url`` (after redirects), ``headers``, ``text``."""

    def __init__(self, reply):
        self.status = reply.get("status", 0)
        self.url = reply.get("url", "")
        self.headers = reply.get("headers", {})
        self.text = reply.get("text", "")

    @property
    def ok(self):
        return 200 <= self.status < 300

    def json(self):
        return _json.loads(self.text)

    def raise_for_status(self):
        if not self.ok:
            raise ToolError(f"The server answered {self.status} for {self.url}.")
        return self


class _Http:
    """Web requests, made by the Chummy app.

    They only work if the user turned on internet access for Chummy and for
    this tool. Requests to this computer or the local network are refused.
    """

    def get(self, url, headers=None):
        return Response(_request("http", method="GET", url=url, headers=headers or {}))

    def post(self, url, data=None, json=None, headers=None):
        headers = dict(headers or {})
        body = data
        if json is not None:
            body = _json.dumps(json)
            headers.setdefault("Content-Type", "application/json")
        if isinstance(body, bytes):
            body = body.decode("utf-8", "replace")
        return Response(
            _request("http", method="POST", url=url, headers=headers, body=body or "")
        )


http = _Http()


def _cwd():
    try:
        _os.chdir(WORKSPACE)
    except OSError:
        pass
