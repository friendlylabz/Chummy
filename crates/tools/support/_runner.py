"""Runs one Chummy tool inside the sandbox.

Chummy starts this with the tool mounted at /tool (tool.py plus its
packages in /tool/site-packages), the agent's workspace at /workspace, and
the Python standard library at /lib. The first line on stdin says what to
do; the last line on stdout is the result. Everything the tool prints goes
to stderr, which Chummy keeps as the tool's log.
"""

import importlib.util
import inspect
import json
import os
import sys
import traceback
import typing

PROTO = sys.__stdout__
sys.stdout = sys.stderr


def send(obj):
    PROTO.write(json.dumps(obj) + "\n")
    PROTO.flush()


def done_ok(result):
    send({"op": "done", "ok": True, "result": result})


def done_err(message, kind="error"):
    send({"op": "done", "ok": False, "kind": kind, "error": message})


def load_tool():
    sys.path.append("/tool/site-packages")
    import chummy

    spec = importlib.util.spec_from_file_location("chummy_tool", "/tool/tool.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    fns = list(chummy._registered)
    if len(fns) != 1:
        raise DefinitionError(
            f"A tool file needs exactly one function marked @chummy.tool (found {len(fns)})."
        )
    return chummy, fns[0]


class DefinitionError(Exception):
    pass


# ------------------------------------------------------------------ schema

def type_schema(t):
    origin = typing.get_origin(t)
    args = typing.get_args(t)
    if t is str:
        return {"type": "string"}
    if t is bool:
        return {"type": "boolean"}
    if t is int:
        return {"type": "integer"}
    if t is float:
        return {"type": "number"}
    if origin is typing.Literal:
        values = list(args)
        s = {"enum": values}
        if all(isinstance(v, str) for v in values):
            s["type"] = "string"
        return s
    if origin in (list, typing.List):
        return {"type": "array", "items": type_schema(args[0]) if args else {}}
    if origin in (typing.Union, getattr(__import__("types"), "UnionType", None)):
        rest = [a for a in args if a is not type(None)]
        if len(rest) == 1:
            return type_schema(rest[0])
    raise DefinitionError(
        f"Parameter type {t!r} isn't supported. Use str, int, float, bool, list[...] or Literal[...]."
    )


def parse_doc(doc):
    """First paragraph → description; `name: text` lines → parameter help."""
    lines = doc.splitlines()
    desc = []
    i = 0
    while i < len(lines) and lines[i].strip():
        desc.append(lines[i].strip())
        i += 1
    params = {}
    current = None
    for line in lines[i:]:
        stripped = line.strip()
        if not stripped:
            current = None
            continue
        if stripped.lower() in ("args:", "arguments:", "parameters:"):
            continue
        head, sep, rest = stripped.partition(":")
        name = head.split("(")[0].strip()
        if sep and name.isidentifier():
            params[name] = rest.strip()
            current = name
        elif current:
            params[current] = (params[current] + " " + stripped).strip()
    return " ".join(desc), params


def describe(fn):
    sig = inspect.signature(fn)
    hints = typing.get_type_hints(fn)
    description, docs = parse_doc(inspect.getdoc(fn) or "")
    if not description:
        raise DefinitionError("The tool function needs a docstring: one short sentence for the model.")
    props = {}
    required = []
    for name, p in sig.parameters.items():
        if p.kind in (p.VAR_POSITIONAL, p.VAR_KEYWORD, p.POSITIONAL_ONLY):
            raise DefinitionError("Tool parameters must be plain named parameters.")
        s = type_schema(hints.get(name, str))
        if docs.get(name):
            s["description"] = docs[name]
        if p.default is p.empty:
            required.append(name)
        else:
            try:
                json.dumps(p.default)
                if p.default is not None:
                    s["default"] = p.default
            except TypeError:
                pass
        props[name] = s
    return {
        "description": description,
        "parameters": {
            "type": "object",
            "properties": props,
            "required": required,
            "additionalProperties": False,
        },
    }


# ------------------------------------------------------------------- calls

def call(chummy, fn, args):
    sig = inspect.signature(fn)
    try:
        bound = sig.bind(**args)
    except TypeError as e:
        return done_err(f"Wrong arguments: {e}.", "arguments")
    chummy._cwd()
    try:
        result = fn(*bound.args, **bound.kwargs)
    except chummy.ToolError as e:
        return done_err(str(e) or "The tool failed.", "tool")
    except SystemExit:
        return done_err("The tool tried to exit instead of returning a result.", "crash")
    except Exception as e:
        traceback.print_exc()
        return done_err(f"{type(e).__name__}: {e}", "crash")
    if inspect.isawaitable(result):
        return done_err("Tool functions can't be async.", "definition")
    if result is None:
        result = "Done."
    try:
        json.dumps(result)
    except TypeError:
        result = str(result)
    done_ok(result)


def main():
    request = json.loads(sys.stdin.readline() or "{}")
    try:
        chummy, fn = load_tool()
        if request.get("mode") == "describe":
            return done_ok(describe(fn))
        return call(chummy, fn, request.get("args") or {})
    except DefinitionError as e:
        return done_err(str(e), "definition")
    except Exception as e:
        traceback.print_exc()
        return done_err(f"The tool couldn't load: {type(e).__name__}: {e}", "definition")


main()
