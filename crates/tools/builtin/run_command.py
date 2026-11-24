# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "run_command"
# title = "Run command"
# version = "1.1.0"
# timeout = 120
# risky = true
# ///
"""Runs a shell-style command line inside the sandbox.

There's no real shell in the WebAssembly sandbox (and Chummy never runs
anything outside it), so this is a small shell of its own: pipes (|),
&&, ||, ;, several lines, redirects (<, >, >>, 2>&1, 2>/dev/null, 2> file),
heredocs (<<EOF), here-strings (<<<), quotes, globs, {a,b} braces, cd, and a
set of common commands written in Python (sed and awk included), plus
`python` itself, which runs scripts in the workspace in this same sandboxed
interpreter. Everything stays inside the agent's workspace, with no network.
There are no variables, loops or $(…): the model is told to use pipes,
xargs, awk or python instead.

Potentially unsafe: the model can delete or overwrite anything in the
workspace in one command, and run code it wrote (until the time limit).
"""

import base64
import difflib
import fnmatch
import glob as _glob
import hashlib
import io
import math
import os
import random
import re
import runpy
import shlex
import shutil
import string
import sys
import time
import traceback

import chummy

MAX_OUT = 30_000
# Shell state for one call: `set -e`, `set -o pipefail`, the last exit code,
# heredoc bodies.
OPTS = {"errexit": False, "pipefail": False, "status": 0}
DOCS = []
# Stands for $? in a word until the command runs.
STATUS = "\x00status\x00"
NO_VARS = (
    "this shell has no variables, loops, if/then, $(…) or $((…)). Chain commands with pipes, && and xargs "
    "(like  find . -name '*.txt' | xargs wc -l ), use awk for per-line logic and sums "
    "(awk 'BEGIN {print 6*7}'), or write a short python script"
)


class Fail(Exception):
    """A command's error: message for stderr, and the exit code."""

    def __init__(self, msg, code=1):
        super().__init__(msg)
        self.code = code


class _Exit(Exception):
    """`exit N`: stops the command line."""

    def __init__(self, code):
        super().__init__(code)
        self.code = code


class _Result:
    def __init__(self, out, err, code):
        self.out, self.err, self.code = out, err, code


class _Diff(str):
    """diff's output, with exit code 1 (files differ)."""


# ------------------------------------------------------------------ paths

def resolve(p):
    """A path the command gave (relative to the current folder) inside the workspace."""
    if p in ("/dev/null",):
        return p
    p = p.replace("\\", "/")
    if p == "~" or p.startswith("~/"):
        p = p[2:] if p.startswith("~/") else "."
    base = os.getcwd()
    if p.startswith("/"):
        if p == chummy.WORKSPACE or p.startswith(chummy.WORKSPACE + "/"):
            full = os.path.normpath(p)
        else:
            raise Fail(f"{p}: that's outside the workspace (use paths inside it, like notes/a.txt)", 1)
    else:
        full = os.path.normpath(os.path.join(base, p))
    if full != chummy.WORKSPACE and not full.startswith(chummy.WORKSPACE + "/"):
        raise Fail(f"{p}: that's outside the workspace", 1)
    return full


def shown(full):
    rel = os.path.relpath(full, os.getcwd())
    return rel


def read_text(path):
    full = resolve(path)
    if full == "/dev/null":
        return ""
    if os.path.isdir(full):
        raise Fail(f"{path}: is a folder")
    try:
        with open(full, encoding="utf-8", errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        raise Fail(f"{path}: no such file")


def read_bytes(path):
    full = resolve(path)
    if os.path.isdir(full):
        raise Fail(f"{path}: is a folder")
    try:
        with open(full, "rb") as f:
            return f.read()
    except FileNotFoundError:
        raise Fail(f"{path}: no such file")


def write_text(path, text, append=False):
    full = resolve(path)
    if full == "/dev/null":
        return
    with open(full, "a" if append else "w", encoding="utf-8") as f:
        f.write(text)


def inputs(files, stdin):
    if not files:
        return [(None, stdin)]
    return [(None, stdin) if f == "-" else (f, read_text(f)) for f in files]


def lines_of(text):
    """Lines without their newlines (a last line without one counts)."""
    return text.split("\n")[:-1] if text.endswith("\n") else (text.split("\n") if text else [])


def joined(lines):
    return "".join(l + "\n" for l in lines)


def flags(args, known, with_value=()):
    """Splits -abc style options from operands. Returns (opts, operands)."""
    opts, rest, i = {}, [], 0
    while i < len(args):
        a = args[i]
        if a == "--":
            rest += args[i + 1:]
            break
        if a.startswith("-") and len(a) > 1 and not re.fullmatch(r"-\d+", a):
            if a in with_value or a[:2] in with_value:
                key = a[:2] if not a.startswith("--") else a
                if len(a) > 2 and not a.startswith("--"):
                    opts[key] = a[2:]
                else:
                    i += 1
                    if i >= len(args):
                        raise Fail(f"option {a} needs a value", 2)
                    opts[key] = args[i]
            elif a.startswith("--"):
                k, eq, v = a.partition("=")
                opts.setdefault(k, []).append(v) if eq else opts.__setitem__(k, True)
            else:
                for j, ch in enumerate(a[1:], 1):
                    if "-" + ch in with_value:
                        v = a[j + 1:]
                        if not v:
                            i += 1
                            if i >= len(args):
                                raise Fail(f"option -{ch} needs a value", 2)
                            v = args[i]
                        opts["-" + ch] = v
                        break
                    if ch not in known:
                        raise Fail(f"unknown option -{ch} (run  help  to see what works)", 2)
                    opts["-" + ch] = True
        else:
            rest.append(a)
        i += 1
    return opts, rest


def count_opt(args, default=10):
    """-n N, -N or -n +N for head and tail."""
    n, rest, i, from_start = default, [], 0, False
    while i < len(args):
        a = args[i]
        if a == "-n" and i + 1 < len(args):
            v = args[i + 1]
            from_start = v.startswith("+")
            n = int(v.lstrip("+"))
            i += 2
            continue
        if re.fullmatch(r"-n[+-]?\d+", a):
            from_start = "+" in a
            n = int(a[2:].lstrip("+"))
        elif re.fullmatch(r"-\d+", a):
            n = int(a[1:])
        else:
            rest.append(a)
        i += 1
    return n, from_start, rest


_ESC = {"n": "\n", "t": "\t", "r": "\r", "a": "\a", "b": "\b", "f": "\f", "v": "\v", "\\": "\\",
        '"': '"', "'": "'", "/": "/", "e": "\x1b"}


def unescape(s):
    r"""\n, \t, \\, \NNN (octal) and \xHH, as in printf, echo -e and awk strings."""
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c != "\\" or i + 1 == len(s):
            out.append(c)
            i += 1
            continue
        d = s[i + 1]
        if d in _ESC:
            out.append(_ESC[d])
            i += 2
        elif d in "01234567":
            m = re.match(r"0?[0-7]{1,3}", s[i + 1:])
            out.append(chr(int(m.group(), 8) & 0xFF))
            i += 1 + len(m.group())
        elif d == "x" and re.match(r"[0-9a-fA-F]", s[i + 2:i + 3]):
            m = re.match(r"[0-9a-fA-F]{1,2}", s[i + 2:])
            out.append(chr(int(m.group(), 16)))
            i += 2 + len(m.group())
        else:
            out.append(c + d)
            i += 2
    return "".join(out)


# POSIX bracket classes, in Python's syntax.
_CLASSES = {
    "alpha": "a-zA-Z", "digit": "0-9", "alnum": "a-zA-Z0-9", "upper": "A-Z", "lower": "a-z",
    "space": r" \t\n\r\f\v", "blank": r" \t", "punct": r"!-/:-@\[-`{-~", "xdigit": "0-9A-Fa-f",
    "word": r"\w", "cntrl": r"\x00-\x1f\x7f", "print": r" -~", "graph": r"!-~",
}


def posix_re(p, basic=False):
    """A POSIX regular expression (basic for grep and sed, extended for -E and awk) in Python's syntax."""
    out, i, n = [], 0, len(p)
    while i < n:
        c = p[i]
        if c == "[":
            j, part = i + 1, ["["]
            if j < n and p[j] == "^":
                part.append("^")
                j += 1
            if j < n and p[j] == "]":
                part.append(r"\]")
                j += 1
            while j < n and p[j] != "]":
                if p.startswith("[:", j):
                    k = p.find(":]", j + 2)
                    if k > 0 and p[j + 2:k] in _CLASSES:
                        part.append(_CLASSES[p[j + 2:k]])
                        j = k + 2
                        continue
                if p[j] == "[":
                    part.append(r"\[")
                elif p[j] == "\\":
                    if basic or j + 1 >= n:
                        part.append(r"\\")
                    else:
                        part.append(p[j:j + 2])
                        j += 1
                else:
                    part.append(p[j])
                j += 1
            if j >= n:
                out.append(r"\[")
                i += 1
                continue
            part.append("]")
            out.append("".join(part))
            i = j + 1
            continue
        if c == "\\" and i + 1 < n:
            d = p[i + 1]
            if basic and d in "(){}+?|":
                out.append(d)
            elif d in "<>":
                out.append(r"\b")
            elif d == "`":
                out.append(r"\A")
            elif d == "'":
                out.append(r"\Z")
            elif d in "wWsSbBdD" or d.isdigit():
                out.append("\\" + d)
            elif d == "n":
                out.append("\n")
            elif d == "t":
                out.append("\t")
            else:
                out.append(re.escape(d))
            i += 2
            continue
        if basic and c in "(){}+?|":
            out.append("\\" + c)
        elif basic and c == "*" and (not out or out[-1] in ("(", "^", "|")):
            out.append(r"\*")
        else:
            out.append(c)
        i += 1
    return "".join(out)


_RX = {}


def compile_re(p, basic=False, icase=False):
    key = (p, basic, icase)
    rx = _RX.get(key)
    if rx is None:
        try:
            rx = re.compile(posix_re(p, basic), re.IGNORECASE if icase else 0)
        except re.error as e:
            raise Fail(f"bad regular expression {p!r}: {e}", 2)
        _RX[key] = rx
    return rx


_FMT = re.compile(r"%([-+ #0']*)(\*|\d+)?(?:\.(\*|\d*))?([a-zA-Z%])")


def format_values(fmt, args, num, text, char):
    """printf-style formatting for awk and the printf command. Returns (text, number of args used)."""
    out, pos, used = [], 0, 0

    def take():
        nonlocal used
        v = args[used] if used < len(args) else None
        used += 1
        return v

    for m in _FMT.finditer(fmt):
        out.append(fmt[pos:m.start()])
        pos = m.end()
        fl, w, p, c = m.groups()
        fl = fl.replace("'", "")
        if c == "%":
            out.append("%")
            continue
        if c not in "diouxXeEfFgGcsb":
            out.append(m.group(0))
            continue
        if w == "*":
            wv = int(num(take()))
            fl, w = (fl + "-", str(-wv)) if wv < 0 else (fl, str(wv))
        if p == "*":
            p = str(max(int(num(take())), 0))
        spec = "%" + fl + (w or "") + ("." + (p or "0") if p is not None else "")
        v = take()
        try:
            if c in "di":
                out.append((spec + "d") % int(num(v)))
            elif c in "ouxX":
                out.append((spec + ("d" if c == "u" else c)) % int(num(v)))
            elif c in "eEfFgG":
                out.append((spec + c) % float(num(v)))
            elif c == "c":
                out.append((spec + "s") % char(v))
            elif c == "b":
                out.append((spec + "s") % unescape(text(v)))
            else:
                out.append((spec + "s") % text(v))
        except (ValueError, OverflowError, TypeError):
            out.append(text(v))
    out.append(fmt[pos:])
    return "".join(out), min(used, len(args))


# ---------------------------------------------------------------- commands

def c_echo(args, stdin):
    nl, esc = True, False
    while args and re.fullmatch(r"-[neE]+", args[0]):
        nl = nl and "n" not in args[0]
        esc = (esc or "e" in args[0]) and not args[0].endswith("E")
        args = args[1:]
    text = " ".join(args)
    return (unescape(text) if esc else text) + ("\n" if nl else "")


def c_printf(args, stdin):
    if not args:
        raise Fail("usage: printf format [arguments]", 2)
    fmt, rest = unescape(args[0]), args[1:]
    errs = []

    def num(v):
        if v is None or v == "":
            return 0
        v = v.strip()
        if v[:1] in "'\"" and len(v) > 1:
            return ord(v[1])
        try:
            return int(v, 0) if re.fullmatch(r"[-+]?(0[xX][0-9a-fA-F]+|[1-9]\d*|0)", v) else float(v)
        except ValueError:
            errs.append(f"printf: {v}: invalid number\n")
            return 0

    def text(v):
        return "" if v is None else v

    def char(v):
        return (v or "")[:1]

    out = []
    while True:
        s, used = format_values(fmt, rest, num, text, char)
        out.append(s)
        if used == 0 or used >= len(rest):
            break
        rest = rest[used:]
    return _Result("".join(out), "".join(errs), 1 if errs else 0)


def c_cat(args, stdin):
    o, files = flags(args, "nbsA")
    out = "".join(t for _, t in inputs(files, stdin))
    if o.get("-s"):
        out = re.sub(r"\n{3,}", "\n\n", out)
    if o.get("-n") or o.get("-b"):
        n, rows = 0, []
        for l in lines_of(out):
            if o.get("-b") and not l:
                rows.append("")
                continue
            n += 1
            rows.append(f"{n:>6}\t{l}")
        out = joined(rows)
    if o.get("-A"):
        out = joined(l.replace("\t", "^I") + "$" for l in lines_of(out))
    return out


def c_head(args, stdin):
    if "-c" in args:
        i = args.index("-c")
        n, files = int(args[i + 1]), args[:i] + args[i + 2:]
        return "".join(t.encode()[:n].decode(errors="ignore") for _, t in inputs(files, stdin))
    n, _, files = count_opt(args)
    many = len(files) > 1
    out = []
    for name, t in inputs(files, stdin):
        if many:
            out.append(("\n" if out else "") + f"==> {name} <==\n")
        out.append("".join(t.splitlines(True)[:n]))
    return "".join(out)


def c_tail(args, stdin):
    if "-f" in args or "-F" in args:
        raise Fail("-f (follow) can't work here: a command runs once and returns", 2)
    n, from_start, files = count_opt(args)
    many = len(files) > 1
    out = []
    for name, t in inputs(files, stdin):
        if many:
            out.append(("\n" if out else "") + f"==> {name} <==\n")
        lines = t.splitlines(True)
        out.append("".join(lines[n - 1:] if from_start else lines[-n:] if n else []))
    return "".join(out)


def c_wc(args, stdin):
    o, files = flags(args, "lwcm")
    if not o:
        o = {"-l": True, "-w": True, "-c": True}
    rows, total = [], [0, 0, 0]

    def picked(counts):
        return [c for c, k in zip(counts, ("-l", "-w", "-c")) if o.get(k) or (k == "-c" and o.get("-m"))]

    for name, t in inputs(files, stdin):
        counts = [t.count("\n"), len(t.split()), len(t) if o.get("-m") else len(t.encode())]
        total = [a + b for a, b in zip(total, counts)]
        rows.append((picked(counts), name))
    if len(files) > 1:
        rows.append((picked(total), "total"))
    width = len(str(max(picked(total)))) if len(files) > 1 or len(picked(total)) > 1 else 1
    return "".join(" ".join(f"{c:>{width}}" for c in cs) + (f" {name}" if name else "") + "\n" for cs, name in rows)


SKIP_DIRS = (".git", "node_modules", "__pycache__", ".venv", "venv")


def c_grep(args, stdin, extended=False, fixed=False):
    pats, plain, i = [], [], 0
    while i < len(args):
        if args[i] in ("-e", "--regexp") and i + 1 < len(args):
            pats.append(args[i + 1])
            i += 2
            continue
        if args[i].startswith("--regexp="):
            pats.append(args[i].split("=", 1)[1])
        else:
            plain.append(args[i])
        i += 1
    o, rest = flags(plain, "inrvclLEFPwxHhoRsqZ", with_value=("-A", "-B", "-C", "-m", "-f"))
    if "-f" in o:
        pats += lines_of(read_text(o["-f"]))
    if not pats:
        if not rest:
            raise Fail("usage: grep [options] pattern [files]", 2)
        pats, rest = [rest[0]], rest[1:]
    pats = [q for p in pats for q in p.split("\n")]
    files = rest
    if o.get("-F") or fixed:
        conv = re.escape
    elif o.get("-P"):
        conv = lambda p: p  # noqa: E731
    else:
        conv = lambda p: posix_re(p, basic=not (o.get("-E") or extended))  # noqa: E731
    pat = "|".join(f"(?:{conv(p)})" for p in pats)
    if o.get("-w"):
        pat = rf"(?<!\w)(?:{pat})(?!\w)"
    if o.get("-x"):
        pat = rf"\A(?:{pat})\Z"
    try:
        rx = re.compile(pat, re.IGNORECASE if o.get("-i") else 0)
    except re.error as e:
        raise Fail(f"bad pattern: {e}", 2)
    after = int(o.get("-A") or o.get("-C") or 0)
    before = int(o.get("-B") or o.get("-C") or 0)
    most = int(o["-m"]) if "-m" in o else None
    includes, excludes = o.get("--include", []), o.get("--exclude", [])
    skip_dirs = set(SKIP_DIRS) | set(o.get("--exclude-dir", []))
    recursive = o.get("-r") or o.get("-R")
    if not files:
        files = ["."] if recursive else ["-"]
    targets, errs = [], []
    for f in files:
        if f == "-":
            targets.append((None, stdin))
            continue
        try:
            full = resolve(f)
        except Fail as e:
            errs.append(f"grep: {e}\n")
            continue
        if os.path.isdir(full):
            if not recursive:
                errs.append(f"grep: {f}: is a folder (use -r)\n")
                continue
            for folder, dirs, names in os.walk(full):
                dirs[:] = sorted(d for d in dirs if not d.startswith(".") and d not in skip_dirs)
                for n in sorted(names):
                    if includes and not any(fnmatch.fnmatch(n, g) for g in includes):
                        continue
                    if any(fnmatch.fnmatch(n, g) for g in excludes):
                        continue
                    p = os.path.join(folder, n)
                    try:
                        with open(p, encoding="utf-8") as fh:
                            targets.append((shown(p) if f == "." else os.path.join(f, os.path.relpath(p, full)), fh.read()))
                    except (UnicodeDecodeError, OSError):
                        pass
        else:
            try:
                targets.append((f, read_text(f)))
            except Fail as e:
                errs.append(f"grep: {e}\n")
    show_name = ((len(targets) > 1 or recursive) and not o.get("-h")) or o.get("-H")
    out, found = [], False

    def prefix(name, i, sep):
        return (f"{name}{sep}" if show_name and name else "") + (f"{i}{sep}" if o.get("-n") else "")

    for name, text in targets:
        lines = text.splitlines()
        hits, last, left = 0, -1, 0
        for i, line in enumerate(lines):
            hit = bool(rx.search(line)) != bool(o.get("-v"))
            if hit and (most is None or hits < most):
                hits += 1
                found = True
                if o.get("-q"):
                    return _Result("", "", 0)
                if o.get("-l") or o.get("-L") or o.get("-c"):
                    continue
                start = max(i - before, last + 1)
                if (before or after) and last >= 0 and start > last + 1:
                    out.append("--")
                for j in range(start, i):
                    out.append(prefix(name, j + 1, "-") + lines[j])
                if o.get("-o") and not o.get("-v"):
                    out.extend(prefix(name, i + 1, ":") + m.group(0) for m in rx.finditer(line) if m.group(0))
                else:
                    out.append(prefix(name, i + 1, ":") + line)
                last, left = i, after
            elif left > 0 and not (o.get("-l") or o.get("-c")):
                out.append(prefix(name, i + 1, "-") + line)
                last, left = i, left - 1
            elif most is not None and hits >= most:
                break
        label = name or "(standard input)"
        if o.get("-l") and hits:
            out.append(label)
        if o.get("-L") and not hits:
            out.append(label)
        if o.get("-c"):
            out.append((f"{name}:" if show_name and name else "") + str(hits))
    if o.get("-L"):
        found = any(not rx.search(t) for _, t in targets)
    code = 0 if found else 1
    if errs and not o.get("-s"):
        code = 2 if not found else code
    return _Result("\n".join(out) + ("\n" if out else ""), "" if o.get("-s") else "".join(errs), code)


def c_ls(args, stdin):
    o, paths = flags(args, "laR1hAF")
    paths = paths or ["."]
    out = []

    def entry(full, name):
        if o.get("-l"):
            st = os.stat(full)
            kind = "d" if os.path.isdir(full) else "-"
            when = time.strftime("%Y-%m-%d %H:%M", time.localtime(st.st_mtime))
            return f"{kind} {st.st_size:>10} {when} {name}{'/' if kind == 'd' else ''}"
        return name + ("/" if os.path.isdir(full) else "")

    def one(full, label):
        if not os.path.exists(full):
            raise Fail(f"{label}: no such file or folder")
        if not os.path.isdir(full):
            out.append(entry(full, label))
            return
        names = sorted(os.listdir(full), key=str.lower)
        if not (o.get("-a") or o.get("-A")):
            names = [n for n in names if not n.startswith(".")]
        if len(paths) > 1 or o.get("-R"):
            out.append(f"{label}:")
        out.extend(entry(os.path.join(full, n), n) for n in names)
        if o.get("-R"):
            for n in names:
                sub = os.path.join(full, n)
                if os.path.isdir(sub):
                    out.append("")
                    one(sub, os.path.join(label, n))

    for p in paths:
        one(resolve(p), p)
    return "\n".join(out) + ("\n" if out else "")


_FIND_HELP = (
    "find knows -name, -iname, -path, -ipath, -regex, -type, -maxdepth, -mindepth, -size, -mtime, -mmin, "
    "-newer, -empty, ! / -not, -o, -a, ( ), -print, -print0, -exec … {} \\; (or +) and -delete"
)


def c_find(args, stdin):
    roots, i = [], 0
    while i < len(args) and not (args[i].startswith("-") or args[i] in ("(", "!", ")")):
        roots.append(args[i])
        i += 1
    roots = roots or ["."]
    toks = args[i:]
    opts = {"maxdepth": None, "mindepth": 0}
    acts = {"any": False}
    batches = []  # -exec … {} + : (words, paths)
    deletes = []
    out, errs = [], []
    pos = 0

    def need(t):
        nonlocal pos
        if pos >= len(toks):
            raise Fail(f"{t} needs a value", 2)
        pos += 1
        return toks[pos - 1]

    def num_test(v, actual):
        """+N: more than N, -N: less than N, N: exactly N."""
        if v.startswith("+"):
            return actual > float(v[1:])
        if v.startswith("-"):
            return actual < float(v[1:])
        return actual == float(v)

    def primary():
        nonlocal pos
        t = toks[pos]
        pos += 1
        if t in ("!", "-not"):
            inner = primary()
            return lambda it: not inner(it)
        if t == "(":
            e = disj()
            if pos >= len(toks) or toks[pos] != ")":
                raise Fail("a ( isn't closed", 2)
            pos += 1
            return e
        if t in ("-name", "-iname"):
            g = need(t)
            if t == "-iname":
                return lambda it: fnmatch.fnmatch(os.path.basename(it[1]).lower(), g.lower())
            return lambda it: fnmatch.fnmatchcase(os.path.basename(it[1]), g)
        if t in ("-path", "-wholename", "-ipath"):
            g = need(t)
            if t == "-ipath":
                return lambda it: fnmatch.fnmatch(it[1].lower(), g.lower())
            return lambda it: fnmatch.fnmatchcase(it[1], g)
        if t in ("-regex", "-iregex"):
            rx = re.compile(need(t), re.IGNORECASE if t == "-iregex" else 0)
            return lambda it: rx.fullmatch(it[1]) is not None
        if t == "-type":
            kinds = need(t).split(",")
            return lambda it: it[2] in kinds
        if t in ("-maxdepth", "-mindepth"):
            opts[t[1:]] = int(need(t))
            return lambda it: True
        if t == "-empty":
            return lambda it: (os.path.isdir(it[0]) and not os.listdir(it[0])) or (
                os.path.isfile(it[0]) and os.path.getsize(it[0]) == 0)
        if t == "-size":
            v = need(t)
            m = re.fullmatch(r"([+-]?)(\d+)([ckMGb]?)", v)
            if not m:
                raise Fail(f"-size {v}: use like +10k, -1M or 0", 2)
            unit = {"c": 1, "k": 1024, "M": 1024 ** 2, "G": 1024 ** 3}.get(m.group(3), 512)
            return lambda it: os.path.isfile(it[0]) and num_test(
                m.group(1) + m.group(2), math.ceil(os.path.getsize(it[0]) / unit))
        if t in ("-mtime", "-mmin"):
            v, per = need(t), 86400 if t == "-mtime" else 60
            return lambda it: num_test(v, math.floor((time.time() - os.path.getmtime(it[0])) / per))
        if t == "-newer":
            ref = os.path.getmtime(resolve(need(t)))
            return lambda it: os.path.getmtime(it[0]) > ref
        if t in ("-print", "-print0"):
            acts["any"] = True
            end = "\0" if t == "-print0" else "\n"
            return lambda it: out.append(it[1] + end) or True
        if t == "-delete":
            acts["any"] = True
            return lambda it: deletes.append(it) or True
        if t in ("-exec", "-execdir", "-ok"):
            acts["any"] = True
            words = []
            while pos < len(toks) and toks[pos] not in (";", "+"):
                words.append(toks[pos])
                pos += 1
            if pos >= len(toks) or not words:
                raise Fail("-exec needs a command and \\; (or +) at the end, like  -exec grep -l TODO {} +", 2)
            end = toks[pos]
            pos += 1
            if end == "+":
                batch = (words, [])
                batches.append(batch)
                return lambda it: batch[1].append(it[1]) or True

            def run(it):
                o, e, code = invoke([w.replace("{}", it[1]) for w in words], "")
                out.append(o)
                errs.append(e)
                return code == 0
            return run
        if t in ("-depth", "-follow", "-xdev", "-mount", "-noleaf"):
            return lambda it: True
        raise Fail(f"{t} isn't supported here ({_FIND_HELP})", 2)

    def conj():
        parts = [primary()]
        while pos < len(toks) and toks[pos] not in ("-o", "-or", ")"):
            if toks[pos] in ("-a", "-and"):
                skip()
                continue
            parts.append(primary())
        return lambda it: all(p(it) for p in parts)

    def skip():
        nonlocal pos
        pos += 1

    def disj():
        parts = [conj()]
        while pos < len(toks) and toks[pos] in ("-o", "-or"):
            skip()
            parts.append(conj())
        return lambda it: any(p(it) for p in parts)

    test = disj() if toks else (lambda it: True)
    if pos < len(toks):
        raise Fail(f"unexpected {toks[pos]} ({_FIND_HELP})", 2)

    def walk(full, label, depth):
        kind = "l" if os.path.islink(full) else "d" if os.path.isdir(full) else "f"
        yield full, label, kind, depth
        if kind == "d" and (opts["maxdepth"] is None or depth < opts["maxdepth"]):
            for n in sorted(os.listdir(full)):
                if n in SKIP_DIRS:
                    continue
                yield from walk(os.path.join(full, n), (label if label.endswith("/") else label + "/") + n, depth + 1)

    for r in roots:
        top = resolve(r)
        if not os.path.exists(top):
            errs.append(f"find: {r}: no such file or folder\n")
            continue
        for full, label, kind, depth in walk(top, r, 0):
            if depth < opts["mindepth"]:
                continue
            it = (full, label, kind)
            if test(it) and not acts["any"]:
                out.append(label + "\n")
    for words, paths in batches:
        if paths:
            cmd = []
            for w in words:
                cmd.extend(paths if w == "{}" else [w])
            if "{}" not in words:
                cmd.extend(paths)
            o, e, code = invoke(cmd, "")
            out.append(o)
            errs.append(e)
    for full, label, kind in sorted(deletes, key=lambda it: -it[0].count("/")):
        if full == chummy.WORKSPACE:
            errs.append("find: refusing to delete the whole workspace\n")
        elif kind == "d":
            try:
                os.rmdir(full)
            except OSError:
                errs.append(f"find: {label}: folder isn't empty\n")
        elif os.path.lexists(full):
            os.remove(full)
    return _Result("".join(out), "".join(errs), 1 if any(errs) else 0)


def c_mkdir(args, stdin):
    o, paths = flags(args, "p")
    for p in paths:
        full = resolve(p)
        if o.get("-p"):
            os.makedirs(full, exist_ok=True)
        else:
            try:
                os.mkdir(full)
            except FileExistsError:
                raise Fail(f"{p}: already exists")
            except FileNotFoundError:
                raise Fail(f"{p}: the folder above doesn't exist (use -p)")
    return ""


def c_rm(args, stdin):
    o, paths = flags(args, "rfRd")
    for p in paths:
        full = resolve(p)
        if full == chummy.WORKSPACE:
            raise Fail("refusing to remove the whole workspace")
        if not os.path.lexists(full):
            if not o.get("-f"):
                raise Fail(f"{p}: no such file or folder")
            continue
        if os.path.isdir(full) and not os.path.islink(full):
            if not (o.get("-r") or o.get("-R")):
                raise Fail(f"{p}: is a folder (use -r)")
            shutil.rmtree(full)
        else:
            os.remove(full)
    return ""


def c_cp(args, stdin):
    o, paths = flags(args, "rRf")
    if len(paths) < 2:
        raise Fail("usage: cp [-r] source... target", 2)
    *srcs, dst = paths
    d = resolve(dst)
    for s in srcs:
        sf = resolve(s)
        target = os.path.join(d, os.path.basename(sf)) if os.path.isdir(d) else d
        if os.path.isdir(sf):
            if not (o.get("-r") or o.get("-R")):
                raise Fail(f"{s}: is a folder (use -r)")
            shutil.copytree(sf, target, dirs_exist_ok=True)
        elif os.path.exists(sf):
            shutil.copyfile(sf, target)
        else:
            raise Fail(f"{s}: no such file")
    return ""


def c_mv(args, stdin):
    o, paths = flags(args, "f")
    if len(paths) < 2:
        raise Fail("usage: mv source... target", 2)
    *srcs, dst = paths
    d = resolve(dst)
    for s in srcs:
        sf = resolve(s)
        if not os.path.exists(sf):
            raise Fail(f"{s}: no such file or folder")
        target = os.path.join(d, os.path.basename(sf)) if os.path.isdir(d) else d
        shutil.move(sf, target)
    return ""


def c_touch(args, stdin):
    for p in args:
        full = resolve(p)
        with open(full, "a", encoding="utf-8"):
            pass
        os.utime(full)
    return ""


def _human(n):
    for unit in ("", "K", "M", "G", "T"):
        if abs(n) < 1024 or unit == "T":
            if unit == "":
                return str(int(n))
            return f"{n:.1f}{unit}" if n < 10 else f"{math.ceil(n)}{unit}"
        n /= 1024
    return str(n)


def _human_num(s):
    m = re.match(r"\s*([-+]?\d+(?:\.\d+)?)\s*([KMGTP]?)", s, re.IGNORECASE)
    if not m:
        return 0.0
    return float(m.group(1)) * 1024 ** "_KMGTP".index((m.group(2) or "_").upper())


def _lead_num(s):
    m = re.match(r"\s*([-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?)", s)
    return float(m.group(1)) if m else 0.0


def c_sort(args, stdin):
    specs, plain, i = [], [], 0
    while i < len(args):
        a = args[i]
        if a in ("-k", "--key") and i + 1 < len(args):
            specs.append(args[i + 1])
            i += 2
            continue
        if re.fullmatch(r"-k\S+", a):
            specs.append(a[2:])
        else:
            plain.append(a)
        i += 1
    o, files = flags(plain, "rnufhVsbdg", with_value=("-t", "-o"))
    lines = [l for _, t in inputs(files, stdin) for l in lines_of(t)]
    sep = unescape(o["-t"]) if "-t" in o else None
    keys = []
    for spec in specs or [None]:
        start = end = None
        kflags = ""
        if spec:
            m = re.fullmatch(r"(\d+)(?:\.\d+)?([a-zA-Z]*)(?:,(\d+)(?:\.\d+)?([a-zA-Z]*))?", spec)
            if not m:
                raise Fail(f"-k {spec}: use like -k2 or -k2,2n", 2)
            start, end = int(m.group(1)), int(m.group(3)) if m.group(3) else None
            kflags = m.group(2) + (m.group(4) or "")
        own = bool(set(kflags) - {"b"})
        keys.append({
            "start": start, "end": end,
            "num": "n" in kflags or "g" in kflags or (not own and (o.get("-n") or o.get("-g"))),
            "human": "h" in kflags or (not own and o.get("-h")),
            "version": "V" in kflags or (not own and o.get("-V")),
            "fold": "f" in kflags or (not own and o.get("-f")),
            "rev": "r" in kflags if own else bool(o.get("-r")),
            "blank": "b" in kflags or o.get("-b"),
        })

    def field(l, k):
        if k["start"] is None:
            return l
        cols = l.split(sep) if sep is not None else l.split()
        part = cols[k["start"] - 1:k["end"]] if k["end"] else cols[k["start"] - 1:]
        return (sep if sep is not None else " ").join(part)

    def one(l, k):
        f = field(l, k)
        if k["blank"]:
            f = f.strip()
        if k["num"]:
            return _lead_num(f)
        if k["human"]:
            return _human_num(f)
        if k["version"]:
            return tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in re.findall(r"\d+|\D+", f))
        return f.lower() if k["fold"] else f

    # Stable sorts from the last key to the first: each key keeps its own direction.
    if not o.get("-s") and (specs or keys[0]["num"] or keys[0]["human"] or keys[0]["version"] or keys[0]["fold"]):
        lines.sort(reverse=bool(o.get("-r")))
    for k in reversed(keys):
        lines.sort(key=lambda l: one(l, k), reverse=k["rev"])
    if o.get("-u"):
        seen, uniq = set(), []
        for l in lines:
            k = tuple(one(l, k) for k in keys)
            if k not in seen:
                seen.add(k)
                uniq.append(l)
        lines = uniq
    if "-o" in o:
        write_text(o["-o"], joined(lines))
        return ""
    return joined(lines)


def c_uniq(args, stdin):
    o, files = flags(args, "cdui")
    out, prev, n = [], None, 0

    def same(a, b):
        return a is not None and b is not None and (a.lower() == b.lower() if o.get("-i") else a == b)

    lines = lines_of(inputs(files[:1], stdin)[0][1])
    for l in lines + [None]:
        if same(l, prev):
            n += 1
            continue
        if prev is not None and not (o.get("-d") and n < 2) and not (o.get("-u") and n > 1):
            out.append(f"{n:>7} {prev}" if o.get("-c") else prev)
        prev, n = l, 1
    if len(files) > 1:
        write_text(files[1], joined(out))
        return ""
    return joined(out)


_TR_CLASSES = {
    "upper": string.ascii_uppercase, "lower": string.ascii_lowercase, "digit": string.digits,
    "alpha": string.ascii_letters, "alnum": string.ascii_letters + string.digits, "space": " \t\n\r\f\v",
    "blank": " \t", "punct": string.punctuation, "xdigit": "0123456789abcdefABCDEF",
}


def _tr_set(s):
    s = unescape(s)
    out, i = [], 0
    while i < len(s):
        if s.startswith("[:", i):
            k = s.find(":]", i)
            if k > 0 and s[i + 2:k] in _TR_CLASSES:
                out.extend(_TR_CLASSES[s[i + 2:k]])
                i = k + 2
                continue
        if i + 2 < len(s) and s[i + 1] == "-":
            out.extend(chr(c) for c in range(ord(s[i]), ord(s[i + 2]) + 1))
            i += 3
            continue
        out.append(s[i])
        i += 1
    return out


def _squeeze(text, chars):
    if not chars:
        return text
    return re.sub("(" + "|".join(map(re.escape, set(chars))) + r")\1+", r"\1", text)


def c_tr(args, stdin):
    o, sets = flags(args, "dscC")
    if not sets:
        raise Fail("usage: tr set1 set2  or  tr -d set  or  tr -s set", 2)
    comp = o.get("-c") or o.get("-C")
    s1 = _tr_set(sets[0])
    keep = set(s1)
    text = stdin
    if o.get("-d"):
        text = "".join(ch for ch in text if (ch in keep) == bool(comp))
        if o.get("-s") and len(sets) > 1:
            text = _squeeze(text, _tr_set(sets[1]))
        return text
    if len(sets) >= 2:
        s2 = _tr_set(sets[1])
        if not s2:
            raise Fail("set2 is empty", 2)
        if comp:
            text = "".join(ch if ch in keep else s2[-1] for ch in text)
        else:
            s2 = (s2 + s2[-1:] * len(s1))[:len(s1)]
            text = text.translate({ord(a): b for a, b in zip(s1, s2)})
        return _squeeze(text, s2) if o.get("-s") else text
    if o.get("-s"):
        return _squeeze(text, s1)
    raise Fail("usage: tr set1 set2  or  tr -d set  or  tr -s set", 2)


def c_tee(args, stdin):
    o, files = flags(args, "a")
    for f in files:
        write_text(f, stdin, append=bool(o.get("-a")))
    return stdin


def c_seq(args, stdin):
    sep, width = "\n", False
    while args and args[0].startswith("-") and not re.fullmatch(r"-[\d.]+", args[0]):
        a = args.pop(0)
        if a == "-s":
            sep = unescape(args.pop(0))
        elif a.startswith("-s"):
            sep = unescape(a[2:])
        elif a == "-w":
            width = True
        else:
            raise Fail(f"unknown option {a}", 2)
    try:
        nums = [float(a) for a in args]
    except ValueError:
        raise Fail("usage: seq [first [step]] last", 2)
    if not 1 <= len(nums) <= 3:
        raise Fail("usage: seq [first [step]] last", 2)
    first, step, last = (1.0, 1.0, nums[0]) if len(nums) == 1 else (
        (nums[0], 1.0, nums[1]) if len(nums) == 2 else tuple(nums))
    if step == 0:
        raise Fail("the step can't be 0", 2)
    count = math.floor((last - first) / step + 1e-9) + 1
    if count > 1_000_000:
        raise Fail("that's more than a million numbers", 2)
    ints = all(float(n).is_integer() for n in nums)
    decimals = max((len(a.split(".")[1]) for a in args if "." in a), default=0)
    vals = [first + i * step for i in range(max(count, 0))]
    text = [str(int(v)) if ints else f"{v:.{decimals}f}" for v in vals]
    if width and text:
        w = max(len(t) for t in text)
        text = [t.zfill(w) for t in text]
    return sep.join(text) + ("\n" if text else "")


def c_basename(args, stdin):
    o, rest = flags(args, "a", with_value=("-s",))
    if not rest:
        raise Fail("usage: basename path [suffix]", 2)
    names = rest if (o.get("-a") or "-s" in o) else rest[:1]
    suffix = o.get("-s") or (rest[1] if len(rest) > 1 and not o.get("-a") else "")
    out = []
    for p in names:
        b = os.path.basename(p.rstrip("/")) or "/"
        if suffix and b.endswith(suffix) and b != suffix:
            b = b[:-len(suffix)]
        out.append(b)
    return joined(out)


def c_dirname(args, stdin):
    if not args:
        raise Fail("usage: dirname path", 2)
    return joined(os.path.dirname(p.rstrip("/")) or "." for p in args)


def c_realpath(args, stdin):
    paths = [a for a in args if not a.startswith("-")]
    if not paths:
        raise Fail("usage: realpath path", 2)
    return joined(resolve(p) for p in paths)


def c_nl(args, stdin):
    o, files = flags(args, "", with_value=("-b", "-w", "-s", "-v"))
    width, sep, n = int(o.get("-w", 6)), unescape(o.get("-s", "\t")), int(o.get("-v", 1))
    every = o.get("-b", "t") == "a"
    out = []
    for _, t in inputs(files, stdin):
        for l in lines_of(t):
            if l.strip() or every:
                out.append(f"{n:>{width}}{sep}{l}")
                n += 1
            else:
                out.append("")
    return joined(out)


def c_tac(args, stdin):
    return joined(l for _, t in reversed(inputs(args, stdin)) for l in reversed(lines_of(t)))


def c_rev(args, stdin):
    return joined(l[::-1] for _, t in inputs(args, stdin) for l in lines_of(t))


def c_paste(args, stdin):
    o, files = flags(args, "s", with_value=("-d",))
    delims = unescape(o.get("-d", "\t")) or "\t"
    cols = [lines_of(t) for _, t in inputs(files or ["-"], stdin)]

    def glue(items):
        s = ""
        for i, item in enumerate(items):
            s += (delims[(i - 1) % len(delims)] if i else "") + item
        return s

    if o.get("-s"):
        return joined(glue(c) for c in cols)
    rows = max((len(c) for c in cols), default=0)
    return joined(glue([c[i] if i < len(c) else "" for c in cols]) for i in range(rows))


def c_column(args, stdin):
    o, files = flags(args, "t", with_value=("-s", "-o"))
    text = "".join(t for _, t in inputs(files, stdin))
    if not o.get("-t"):
        return text
    sep = unescape(o["-s"]) if "-s" in o else None
    rows = [(re.split("[" + re.escape(sep) + "]", l) if sep else l.split()) for l in lines_of(text) if l.strip()]
    widths = {}
    for r in rows:
        for i, c in enumerate(r):
            widths[i] = max(widths.get(i, 0), len(c))
    gap = unescape(o.get("-o", "  "))
    return joined(gap.join(c.ljust(widths[i]) if i < len(r) - 1 else c for i, c in enumerate(r)) for r in rows)


def c_tree(args, stdin):
    o, paths = flags(args, "adf", with_value=("-L",))
    depth = int(o["-L"]) if "-L" in o else None
    out, counts = [], [0, 0]

    def walk(full, pre, level):
        names = sorted(os.listdir(full), key=str.lower)
        names = [n for n in names if (o.get("-a") or not n.startswith(".")) and n not in SKIP_DIRS]
        if o.get("-d"):
            names = [n for n in names if os.path.isdir(os.path.join(full, n))]
        for i, n in enumerate(names):
            p = os.path.join(full, n)
            last = i == len(names) - 1
            is_dir = os.path.isdir(p)
            counts[0 if is_dir else 1] += 1
            out.append(pre + ("└── " if last else "├── ") + n + ("/" if is_dir else ""))
            if is_dir and (depth is None or level < depth):
                walk(p, pre + ("    " if last else "│   "), level + 1)

    for p in paths or ["."]:
        full = resolve(p)
        if not os.path.isdir(full):
            raise Fail(f"{p}: not a folder")
        out.append(p)
        walk(full, "", 1)
    out.append("")
    out.append(f"{counts[0]} director{'y' if counts[0] == 1 else 'ies'}, {counts[1]} file{'' if counts[1] == 1 else 's'}")
    return joined(out)


def c_du(args, stdin):
    o, paths = flags(args, "shac", with_value=("-d",))
    if o.get("--max-depth"):
        o["-d"] = o["--max-depth"][0]
    limit = int(o["-d"]) if "-d" in o else (0 if o.get("-s") else None)
    out, grand = [], 0

    def size_text(n):
        return _human(n) if o.get("-h") else str(math.ceil(n / 1024))

    def walk(full, label, level):
        if not os.path.isdir(full) or os.path.islink(full):
            n = os.path.getsize(full) if os.path.exists(full) else 0
            if level == 0 or o.get("-a"):
                if limit is None or level <= limit:
                    out.append(f"{size_text(n)}\t{label}")
            return n
        total = 0
        for name in sorted(os.listdir(full)):
            total += walk(os.path.join(full, name), (label.rstrip("/") + "/" + name), level + 1)
        if limit is None or level <= limit:
            out.append(f"{size_text(total)}\t{label}")
        return total

    for p in paths or ["."]:
        grand += walk(resolve(p), p, 0)
    if o.get("-c"):
        out.append(f"{size_text(grand)}\ttotal")
    return joined(out)


def _test(a):
    if not a:
        return False
    if a[0] == "!":
        return not _test(a[1:])
    for op in ("-o", "-a"):
        if op in a[1:-1]:
            i = a.index(op, 1)
            l, r = _test(a[:i]), _test(a[i + 1:])
            return (l or r) if op == "-o" else (l and r)
    if len(a) == 1:
        return a[0] != ""
    if len(a) == 2:
        op, x = a
        if op == "-n":
            return x != ""
        if op == "-z":
            return x == ""
        if op in ("-e", "-f", "-d", "-s", "-r", "-w", "-x", "-L", "-h"):
            try:
                full = resolve(x)
            except Fail:
                return False
            return {
                "-e": os.path.exists, "-f": os.path.isfile, "-d": os.path.isdir, "-L": os.path.islink,
                "-h": os.path.islink, "-r": os.path.exists, "-w": os.path.exists, "-x": os.path.exists,
                "-s": lambda p: os.path.isfile(p) and os.path.getsize(p) > 0,
            }[op](full)
        raise Fail(f"{op}: unknown test (try -e, -f, -d, -s, -n, -z)", 2)
    if len(a) == 3:
        x, op, y = a
        if op in ("=", "=="):
            return x == y
        if op == "!=":
            return x != y
        if op in ("<", ">"):
            return x < y if op == "<" else x > y
        if op in ("-eq", "-ne", "-lt", "-le", "-gt", "-ge"):
            try:
                i, j = int(x), int(y)
            except ValueError:
                raise Fail("integer expression expected", 2)
            return {"-eq": i == j, "-ne": i != j, "-lt": i < j, "-le": i <= j, "-gt": i > j, "-ge": i >= j}[op]
        if op in ("-nt", "-ot"):
            fx, fy = resolve(x), resolve(y)
            tx = os.path.getmtime(fx) if os.path.exists(fx) else -1
            ty = os.path.getmtime(fy) if os.path.exists(fy) else -1
            return tx > ty if op == "-nt" else tx < ty
        raise Fail(f"{op}: unknown test", 2)
    raise Fail("too many arguments (quote strings that have spaces)", 2)


def c_test(args, stdin):
    if not _test(args):
        raise Fail("", 1)
    return ""


def c_bracket(args, stdin, close="]"):
    if not args or args[-1] != close:
        raise Fail(f"missing {close} at the end", 2)
    return c_test(args[:-1], stdin)


def c_which(args, stdin):
    names = [a for a in args if not a.startswith("-")]
    found = [n for n in names if n in COMMANDS]
    missing = [n for n in names if n not in COMMANDS]
    return _Result(joined(f"{n}: built into this shell" for n in found),
                   joined(f"which: no {n} here" for n in missing), 1 if missing or not names else 0)


def c_type(args, stdin):
    found = [n for n in args if n in COMMANDS]
    missing = [n for n in args if n not in COMMANDS]
    return _Result(joined(f"{n} is built into this shell" for n in found),
                   joined(f"type: {n}: not found" for n in missing), 1 if missing else 0)


def c_command(args, stdin):
    if args[:1] in (["-v"], ["-V"]):
        found = [n for n in args[1:] if n in COMMANDS]
        return _Result(joined(found), "", 0 if len(found) == len(args[1:]) else 1)
    if not args:
        return ""
    return _Result(*invoke(args, stdin))


def _hashsum(algo):
    def c(args, stdin):
        o, files = flags(args, "bt")
        out = []
        for f in files or ["-"]:
            data = stdin.encode() if f == "-" else read_bytes(f)
            try:
                h = hashlib.new(algo, data).hexdigest()
            except ValueError:
                raise Fail(f"{algo} isn't available in this Python", 1)
            out.append(f"{h}  {f}")
        return joined(out)
    return c


def c_base64(args, stdin):
    o, files = flags(args, "di", with_value=("-w",))
    if o.get("--decode"):
        o["-d"] = True
    data = stdin.encode() if not files or files == ["-"] else read_bytes(files[0])
    if o.get("-d"):
        try:
            return base64.b64decode(b"".join(data.split()), validate=False).decode("utf-8", errors="replace")
        except ValueError as e:
            raise Fail(f"invalid input: {e}")
    text = base64.b64encode(data).decode()
    w = int(o.get("-w", 76))
    if w:
        text = "\n".join(text[i:i + w] for i in range(0, len(text), w))
    return text + "\n"


def c_xargs(args, stdin):
    n = repl = delim = None
    trace = False
    i = 0
    while i < len(args) and args[i].startswith("-"):
        a = args[i]
        if a in ("-n", "-L", "-P", "-s", "-d", "-E") and i + 1 < len(args):
            v = args[i + 1]
            i += 2
            if a in ("-n", "-L"):
                n = int(v)
            elif a == "-d":
                delim = unescape(v)
            continue
        if re.fullmatch(r"-[nLP]\d+", a):
            if a[1] in "nL":
                n = int(a[2:])
        elif a == "-I" and i + 1 < len(args):
            repl = args[i + 1]
            i += 1
        elif a.startswith("-I"):
            repl = a[2:]
        elif a == "-i":
            repl = "{}"
        elif a in ("-0", "--null"):
            delim = "\0"
        elif a == "-t":
            trace = True
        elif a in ("-r", "--no-run-if-empty", "-x"):
            pass
        else:
            raise Fail(f"unknown option {a}", 2)
        i += 1
    cmd = args[i:] or ["echo"]
    if delim is not None:
        items = [x for x in stdin.split(delim) if x != ""]
        if delim != "\n" and items and items[-1].endswith("\n"):
            items[-1] = items[-1][:-1]
    elif repl is not None or (n is not None and "-L" in args):
        items = [l.strip() for l in lines_of(stdin) if l.strip()]
    else:
        try:
            items = shlex.split(stdin)
        except ValueError:
            items = stdin.split()
    if not items:
        return ""
    if repl is not None:
        batches = [[w.replace(repl, item) for w in cmd] for item in items]
    elif n:
        batches = [cmd + items[j:j + n] for j in range(0, len(items), n)]
    else:
        batches = [cmd + items]
    out, err, code = [], [], 0
    for words in batches:
        if trace:
            err.append(" ".join(words) + "\n")
        o, e, c = invoke(words, "")
        out.append(o)
        err.append(e)
        if c == 127:
            return _Result("".join(out), "".join(err), 127)
        if c:
            code = 123
    return _Result("".join(out), "".join(err), code)


def c_set(args, stdin):
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("-o", "+o") and i + 1 < len(args):
            name = args[i + 1]
            if name in ("errexit", "pipefail"):
                OPTS[name] = a == "-o"
            i += 2
            continue
        if re.fullmatch(r"[-+][euxvo]+", a):
            if "e" in a:
                OPTS["errexit"] = a[0] == "-"
            if a.endswith("o") and i + 1 < len(args):
                if args[i + 1] in ("errexit", "pipefail"):
                    OPTS[args[i + 1]] = a[0] == "-"
                i += 1
        i += 1
    return ""


def c_exit(args, stdin):
    raise _Exit(int(args[0]) if args and re.fullmatch(r"-?\d+", args[0]) else OPTS["status"])


def c_chmod(args, stdin):
    """Files in the sandbox have no permissions to change: accepted, does nothing."""
    return ""


def c_sleep(args, stdin):
    time.sleep(min(float(args[0].rstrip("s")) if args else 0, 60))
    return ""


def c_date(args, stdin):
    fmt = next((a[1:] for a in args if a.startswith("+")), "%a %b %d %H:%M:%S %Z %Y")
    utc = "-u" in args
    return time.strftime(fmt, time.gmtime() if utc else time.localtime()) + "\n"


def c_shell(args, stdin):
    """bash -c "…", sh -c "…", bash script.sh: run it here."""
    i = 0
    while i < len(args) and args[i].startswith("-") and args[i] != "-c":
        i += 2 if args[i] in ("-o", "+o") else 1
    rest = args[i:]
    if rest[:1] == ["-c"] and len(rest) >= 2:
        return _Result(*run_line(rest[1], stdin))
    if rest and not rest[0].startswith("-"):
        return _Result(*run_line(read_text(rest[0]), stdin))
    if not rest and stdin.strip():
        return _Result(*run_line(stdin, ""))
    raise Fail("only  bash -c \"command\"  or  bash script.sh  work here (this is Chummy's own small shell)", 2)


def c_cut(args, stdin):
    o, files = flags(args, "", with_value=("-d", "-f", "-c"))
    def ranges(spec):
        out = []
        for part in spec.split(","):
            a, _, b = part.partition("-")
            lo = int(a) if a else 1
            hi = int(b) if b else (lo if not _ else 10**9)
            out.append((lo, hi))
        return out
    out = []
    for _, t in inputs(files, stdin):
        for l in t.splitlines():
            if "-c" in o:
                out.append("".join(l[lo - 1:hi] for lo, hi in ranges(o["-c"])))
            elif "-f" in o:
                d = o.get("-d", "\t")
                cols = l.split(d)
                out.append(d.join(c for lo, hi in ranges(o["-f"]) for c in cols[lo - 1:hi]))
            else:
                raise Fail("give -f or -c", 2)
    return "\n".join(out) + ("\n" if out else "")


def c_diff(args, stdin):
    o, files = flags(args, "u")
    if len(files) != 2:
        raise Fail("usage: diff file1 file2", 2)
    a, b = read_text(files[0]), read_text(files[1])
    d = list(difflib.unified_diff(a.splitlines(True), b.splitlines(True), files[0], files[1]))
    if not d:
        return ""
    return _Diff("".join(d))


def c_pwd(args, stdin):
    rel = os.path.relpath(os.getcwd(), chummy.WORKSPACE)
    return chummy.WORKSPACE + ("" if rel == "." else "/" + rel) + "\n"


def c_cd(args, stdin):
    target = resolve(args[0] if args else ".")
    if not os.path.isdir(target):
        raise Fail(f"{args[0]}: no such folder")
    os.chdir(target)
    return ""


def c_true(args, stdin):
    return ""


def c_false(args, stdin):
    raise Fail("", 1)


def c_python(args, stdin):
    """python file.py args | python -c code | python -m module args | python (script on stdin)."""
    out, err = io.StringIO(), io.StringIO()
    saved = sys.argv, sys.stdin, sys.stdout, sys.stderr, list(sys.path)
    code = 0
    try:
        sys.stdin, sys.stdout, sys.stderr = io.StringIO(stdin), out, err
        if args[:1] == ["-c"] and len(args) > 1:
            sys.argv = ["-c"] + args[2:]
            sys.path.insert(0, os.getcwd())
            exec(compile(args[1], "<string>", "exec"), {"__name__": "__main__"})
        elif args[:1] == ["-m"] and len(args) > 1:
            sys.argv = [args[1]] + args[2:]
            sys.path.insert(0, os.getcwd())
            runpy.run_module(args[1], run_name="__main__", alter_sys=True)
        elif args and args[0] not in ("-", ""):
            script = resolve(args[0])
            if not os.path.isfile(script):
                raise Fail(f"can't open file {args[0]}: no such file", 2)
            sys.argv = [args[0]] + args[1:]
            sys.path.insert(0, os.path.dirname(script))
            runpy.run_path(script, run_name="__main__")
        else:
            sys.argv = ["-"]
            sys.stdin = io.StringIO("")
            exec(compile(stdin, "<stdin>", "exec"), {"__name__": "__main__"})
    except SystemExit as e:
        if e.code is None:
            code = 0
        elif isinstance(e.code, int):
            code = e.code
        else:
            err.write(str(e.code) + "\n")
            code = 1
    except Fail:
        raise
    except BaseException:
        tb = traceback.format_exc().splitlines()
        # Drop this shell's own frames.
        keep = [l for l in tb if "run_command" not in l and "runpy" not in l]
        err.write("\n".join(keep) + "\n")
        code = 1
    finally:
        sys.argv, sys.stdin, sys.stdout, sys.stderr = saved[:4]
        sys.path[:] = saved[4]
    return _Result(out.getvalue(), err.getvalue(), code)


def c_pip(args, stdin):
    raise Fail("packages can't be installed in this sandbox; only Python's standard library is available", 127)


# -------------------------------------------------------------------- sed

class _SedCmd:
    __slots__ = ("a1", "a2", "neg", "cmd", "arg", "active", "end", "target", "label")

    def __init__(self):
        self.a1 = self.a2 = self.arg = self.target = self.label = None
        self.neg = self.active = False
        self.end = 0
        self.cmd = ""


def _sed_parse(script, ext):
    cmds, labels, opened = [], {}, []
    i, n = 0, len(script)
    last_re = [None]

    def peek():
        return script[i] if i < n else ""

    def skip_ws(newlines=False):
        nonlocal i
        while i < n and (script[i] in " \t" or (newlines and script[i] in "\n;")):
            i += 1

    def delimited(delim, what):
        """Text up to an unescaped delim; \\delim becomes delim."""
        nonlocal i
        buf, inbr = [], False
        while i < n:
            c = script[i]
            if c == "\\" and i + 1 < n:
                d = script[i + 1]
                if d == delim:
                    buf.append(delim)
                elif d == "n" and not inbr:
                    buf.append("\\n")
                else:
                    buf.append(c + d)
                i += 2
                continue
            if c == delim and not inbr:
                i += 1
                return "".join(buf)
            if c == "\n" and what == "regex":
                break
            buf.append(c)
            i += 1
        raise Fail(f"unterminated {what} in {script!r}", 1)

    def regex(text, icase=False):
        if text == "":
            if last_re[0] is None:
                raise Fail("no previous regular expression", 1)
            return last_re[0]
        rx = compile_re(text, basic=not ext, icase=icase)
        last_re[0] = rx
        return rx

    def address():
        nonlocal i
        c = peek()
        if c.isdigit():
            m = re.compile(r"(\d+)(?:~(\d+))?").match(script, i)
            i = m.end()
            if m.group(2) is not None:
                return ("step", int(m.group(1)), int(m.group(2)))
            return ("line", int(m.group(1)))
        if c == "$":
            i += 1
            return ("last",)
        if c in "/\\":
            if c == "\\":
                i += 1
            delim = script[i]
            i += 1
            text = delimited(delim, "regex")
            icase = False
            while peek() in ("I", "M"):
                icase = icase or peek() == "I"
                i += 1
            return ("re", regex(text, icase))
        return None

    def rest_of_line():
        nonlocal i
        j = script.find("\n", i)
        j = n if j < 0 else j
        s = script[i:j]
        i = j
        return s

    def text_arg():
        """a/i/c text: one-liner (a text) or classic (a\\ then lines ending in \\)."""
        nonlocal i
        skip_ws()
        if script.startswith("\\\n", i):
            i += 2
        elif peek() == "\\":
            i += 1
        parts = []
        while True:
            line = rest_of_line()
            if line.endswith("\\") and i < n:
                parts.append(line[:-1])
                i += 1
                continue
            parts.append(line)
            break
        return unescape("\n".join(parts).lstrip(" \t") if parts else "")

    while True:
        skip_ws(newlines=True)
        if i >= n:
            break
        if peek() == "#":
            rest_of_line()
            continue
        c = _SedCmd()
        c.a1 = address()
        if c.a1 is not None:
            skip_ws()
            if peek() == ",":
                i += 1
                skip_ws()
                if peek() == "+" or peek() == "~":
                    m = re.compile(r"[+~](\d+)").match(script, i)
                    if not m:
                        raise Fail("bad address after ,", 1)
                    i = m.end()
                    c.a2 = ("plus", int(m.group(1)))
                else:
                    c.a2 = address()
                    if c.a2 is None:
                        raise Fail("bad address after ,", 1)
        skip_ws()
        if peek() == "!":
            c.neg = True
            i += 1
            skip_ws()
        if i >= n:
            raise Fail("missing command", 1)
        k = script[i]
        i += 1
        c.cmd = k
        if k == "{":
            opened.append(len(cmds))
            cmds.append(c)
            continue
        if k == "}":
            if not opened:
                raise Fail("unexpected }", 1)
            cmds[opened.pop()].target = len(cmds)
            continue
        if k == "s":
            delim = script[i]
            i += 1
            pat = delimited(delim, "regex")
            repl = delimited(delim, "replacement")
            g, nth, p, icase, wfile = False, 1, False, False, None
            while i < n and script[i] not in ";\n}":
                f = script[i]
                if f == "g":
                    g = True
                elif f == "p":
                    p = True
                elif f in "iI":
                    icase = True
                elif f.isdigit():
                    m = re.compile(r"\d+").match(script, i)
                    nth = int(m.group())
                    i = m.end()
                    continue
                elif f == "w":
                    i += 1
                    wfile = rest_of_line().strip()
                    break
                elif f in " \t":
                    pass
                elif f in "mM":
                    pass
                else:
                    raise Fail(f"unknown option to s: {f}", 1)
                i += 1
            c.arg = (regex(pat, icase), _sed_repl(repl), g, nth, p, wfile)
        elif k == "y":
            delim = script[i]
            i += 1
            src = unescape(delimited(delim, "y source"))
            dst = unescape(delimited(delim, "y destination"))
            if len(src) != len(dst):
                raise Fail("y: the two sets need the same length", 1)
            c.arg = str.maketrans(src, dst)
        elif k in "aic":
            c.arg = text_arg()
        elif k == ":":
            if c.a1 is not None:
                raise Fail(": doesn't take an address", 1)
            m = re.compile(r"\s*([^;\n]*)").match(script, i)
            i = m.end()
            labels[m.group(1).strip()] = len(cmds)
            continue
        elif k in "btT":
            m = re.compile(r"[ \t]*([^;\n}]*)").match(script, i)
            i = m.end()
            c.label = m.group(1).strip()
        elif k in "rRwW":
            c.arg = rest_of_line().strip()
        elif k in "qQ":
            m = re.compile(r"[ \t]*(\d*)").match(script, i)
            i = m.end()
            c.arg = int(m.group(1)) if m.group(1) else 0
        elif k in "dDpPnNhHgGxz=lF":
            pass
        else:
            raise Fail(f"unknown command: {k!r} (run  help sed)", 1)
        cmds.append(c)
        skip_ws()
        if i < n and script[i] not in ";\n}#":
            raise Fail(f"extra characters after command {k!r}", 1)
    if opened:
        raise Fail("a { isn't closed", 1)
    for c in cmds:
        if c.cmd in "btT":
            if c.label and c.label not in labels:
                raise Fail(f"can't find label for jump to {c.label!r}", 1)
            c.target = labels[c.label] if c.label else len(cmds)
    return cmds


def _sed_repl(raw):
    parts, i = [], 0
    while i < len(raw):
        c = raw[i]
        if c == "\\" and i + 1 < len(raw):
            d = raw[i + 1]
            if d.isdigit():
                parts.append(int(d))
            elif d in "ULEul":
                parts.append(("case", d))
            elif d in _ESC and d not in "\\&":
                parts.append(_ESC[d])
            elif d == "\n":
                parts.append("\n")
            else:
                parts.append(d)
            i += 2
            continue
        parts.append(0 if c == "&" else c)
        i += 1
    return parts


def _sed_expand(parts, m):
    out, mode, one = [], None, None
    for p in parts:
        if isinstance(p, tuple):
            d = p[1]
            if d in "UL":
                mode = d
            elif d == "E":
                mode = one = None
            else:
                one = d
            continue
        if isinstance(p, int):
            try:
                s = m.group(p) or ""
            except IndexError:
                raise Fail(f"invalid reference \\{p} on s command's RHS (the pattern has fewer groups)", 1)
        else:
            s = p
        if mode == "U":
            s = s.upper()
        elif mode == "L":
            s = s.lower()
        if one and s:
            s = (s[0].upper() if one == "u" else s[0].lower()) + s[1:]
            one = None
        out.append(s)
    return "".join(out)


def _sed_addr(a, ln, ps, last):
    k = a[0]
    if k == "line":
        return ln == a[1]
    if k == "last":
        return last
    if k == "re":
        return a[1].search(ps) is not None
    if k == "step":
        return ln == a[1] if a[2] <= 0 else ln >= a[1] and (ln - a[1]) % a[2] == 0
    return False


def _sed_selected(c, ln, ps, last):
    if c.a1 is None:
        r = True
    elif c.a2 is None:
        r = _sed_addr(c.a1, ln, ps, last)
    elif not c.active:
        r = _sed_addr(c.a1, ln, ps, last)
        if r:
            k = c.a2[0]
            if k == "line":
                c.active = c.a2[1] > ln
            elif k == "plus":
                c.end = ln + c.a2[1]
                c.active = c.a2[1] > 0
            elif k == "last":
                c.active = not last
            else:
                c.active = True
    else:
        r = True
        k = c.a2[0]
        if k == "line":
            done = ln >= c.a2[1]
        elif k == "plus":
            done = ln >= c.end
        else:
            done = _sed_addr(c.a2, ln, ps, last)
        if done:
            c.active = False
    return r != c.neg


def _sed_run(cmds, lines, quiet, out, wfiles):
    idx, ln, hold, code = 0, 0, "", 0
    n = len(lines)
    ps, restart, tflag = "", False, False

    def write(name, text):
        f = wfiles.get(name)
        if f is None:
            if name in ("/dev/stdout", "-"):
                out.append(text)
                return
            f = wfiles[name] = open(resolve(name), "w", encoding="utf-8")
        f.write(text)

    while True:
        if not restart:
            if idx >= n:
                break
            ps = lines[idx]
            idx += 1
            ln += 1
            tflag = False
        restart = False
        append, delete, quit_ = [], False, False
        pc = 0
        while pc < len(cmds):
            c = cmds[pc]
            if not _sed_selected(c, ln, ps, idx >= n):
                pc = c.target if c.cmd == "{" else pc + 1
                continue
            k = c.cmd
            pc += 1
            if k == "{":
                continue
            if k == "s":
                rx, repl, g, nth, p, wfile = c.arg
                pieces, last, count, made = [], 0, 0, False
                for m in rx.finditer(ps):
                    count += 1
                    if count == nth or (g and count > nth):
                        pieces.append(ps[last:m.start()])
                        pieces.append(_sed_expand(repl, m))
                        last = m.end()
                        made = True
                        if not g:
                            break
                if made:
                    pieces.append(ps[last:])
                    ps = "".join(pieces)
                    tflag = True
                    if p:
                        out.append(ps + "\n")
                    if wfile:
                        write(wfile, ps + "\n")
            elif k == "p":
                out.append(ps + "\n")
            elif k == "d":
                delete = True
                break
            elif k == "D":
                if "\n" not in ps:
                    delete = True
                else:
                    ps = ps.split("\n", 1)[1]
                    restart = delete = True
                break
            elif k == "P":
                out.append(ps.split("\n", 1)[0] + "\n")
            elif k == "n":
                if idx < n:
                    if not quiet:
                        out.append(ps + "\n")
                    out.extend(append)
                    append = []
                    ps = lines[idx]
                    idx += 1
                    ln += 1
                else:
                    quit_ = True
                    break
            elif k == "N":
                if idx < n:
                    ps += "\n" + lines[idx]
                    idx += 1
                    ln += 1
                else:
                    quit_ = True
                    break
            elif k == "h":
                hold = ps
            elif k == "H":
                hold += "\n" + ps
            elif k == "g":
                ps = hold
            elif k == "G":
                ps += "\n" + hold
            elif k == "x":
                ps, hold = hold, ps
            elif k == "z":
                ps = ""
            elif k == "=":
                out.append(f"{ln}\n")
            elif k == "F":
                out.append("-\n")
            elif k == "l":
                out.append(ps.encode("unicode_escape").decode() + "$\n")
            elif k == "a":
                append.append(c.arg + "\n")
            elif k == "i":
                out.append(c.arg + "\n")
            elif k == "c":
                if c.a2 is None or c.neg or not c.active:
                    out.append(c.arg + "\n")
                delete = True
                break
            elif k in "rR":
                try:
                    t = read_text(c.arg)
                    append.append(t if t.endswith("\n") or not t else t + "\n")
                except Fail:
                    pass
            elif k in "wW":
                write(c.arg, (ps if k == "w" else ps.split("\n", 1)[0]) + "\n")
            elif k == "y":
                ps = ps.translate(c.arg)
            elif k == "b":
                pc = c.target
            elif k == "t":
                if tflag:
                    tflag = False
                    pc = c.target
            elif k == "T":
                if not tflag:
                    pc = c.target
                else:
                    tflag = False
            elif k in "qQ":
                quit_, code = True, c.arg
                delete = k == "Q"
                break
        if not delete and not quiet:
            out.append(ps + "\n")
        out.extend(append)
        if quit_:
            break
    return code


def c_sed(args, stdin):
    quiet = ext = inplace = False
    suffix, scripts, files = "", [], []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--":
            files += args[i + 1:]
            break
        if a in ("-n", "--quiet", "--silent"):
            quiet = True
        elif a in ("-E", "-r", "--regexp-extended"):
            ext = True
        elif a == "-e" or a == "--expression":
            if i + 1 >= len(args):
                raise Fail("-e needs a script", 2)
            scripts.append(args[i + 1])
            i += 1
        elif a.startswith("--expression="):
            scripts.append(a.split("=", 1)[1])
        elif a == "-f":
            scripts.append(read_text(args[i + 1]))
            i += 1
        elif a in ("-i", "--in-place"):
            inplace = True
            if i + 1 < len(args) and args[i + 1] == "":
                i += 1
        elif a.startswith("--in-place="):
            inplace, suffix = True, a.split("=", 1)[1]
        elif re.fullmatch(r"-i\.\S*", a) or (a.startswith("-i") and not re.fullmatch(r"-i[nEsur]*", a)):
            inplace, suffix = True, a[2:]
        elif re.fullmatch(r"-[nEsuri]+e?", a):
            quiet = quiet or "n" in a
            ext = ext or "E" in a or "r" in a
            inplace = inplace or "i" in a
            if a.endswith("e"):
                scripts.append(args[i + 1])
                i += 1
        elif a in ("-s", "-u", "--posix", "--debug", "-z"):
            pass
        elif a.startswith("-") and a != "-":
            raise Fail(f"unknown option {a} (run  help sed)", 2)
        else:
            files.append(a)
        i += 1
    if not scripts:
        if not files:
            raise Fail("usage: sed [-n] [-E] [-i] 'script' [files]", 2)
        scripts = [files.pop(0)]
    cmds = _sed_parse("\n".join(scripts), ext)
    wfiles = {}
    try:
        if inplace:
            if not files:
                raise Fail("-i needs a file to change", 2)
            for f in files:
                text = read_text(f)
                for c in cmds:
                    c.active = False
                out = []
                _sed_run(cmds, lines_of(text), quiet, out, wfiles)
                result = "".join(out)
                if text and not text.endswith("\n") and result.endswith("\n"):
                    result = result[:-1]
                if suffix:
                    backup = suffix.replace("*", os.path.basename(f)) if "*" in suffix else f + suffix
                    shutil.copyfile(resolve(f), resolve(backup))
                write_text(f, result)
            return ""
        texts = [t for _, t in inputs(files, stdin)]
        lines = [l for t in texts for l in lines_of(t)]
        out = []
        code = _sed_run(cmds, lines, quiet, out, wfiles)
        result = "".join(out)
        if texts and texts[-1] and not texts[-1].endswith("\n") and result.endswith("\n"):
            result = result[:-1]
        return _Result(result, "", code)
    finally:
        for fh in wfiles.values():
            fh.close()


# -------------------------------------------------------------------- awk
#
# A tree-walking interpreter for POSIX awk (plus a few gawk extras: match()
# with an array, ** and **=, length(array), nextfile). No getline, system()
# or pipes to commands: awk reads its input and prints, and the shell
# around it does the rest.

_NUM_FULL = re.compile(r"\s*[-+]?(?:\d+\.?\d*(?:[eE][-+]?\d+)?|\.\d+(?:[eE][-+]?\d+)?)\s*\Z")
_NUM_HEAD = re.compile(r"\s*([-+]?(?:\d+\.?\d*(?:[eE][-+]?\d+)?|\.\d+(?:[eE][-+]?\d+)?))")


class _StrNum(str):
    """Text from the input (fields, -v values, split pieces): a number when it looks like one."""


class _Uninit(str):
    """A variable never set: "" as text, 0 as a number."""


UNSET = _Uninit("")


def a_num(v):
    if isinstance(v, float):
        return v
    m = _NUM_HEAD.match(v)
    return float(m.group(1)) if m else 0.0


def a_isnum(v):
    return isinstance(v, (float, _Uninit)) or (isinstance(v, _StrNum) and _NUM_FULL.match(v) is not None)


def a_str(v, fmt="%.6g"):
    if isinstance(v, float):
        if v != v:
            return "nan"
        if v in (math.inf, -math.inf):
            return "inf" if v > 0 else "-inf"
        if v.is_integer() and abs(v) < 1e16:
            return str(int(v))
        return fmt % v
    return v


def a_bool(v):
    if isinstance(v, float):
        return v != 0
    if isinstance(v, _StrNum) and _NUM_FULL.match(v):
        return a_num(v) != 0
    return v != ""


_AWK_KW = {"BEGIN", "END", "function", "func", "if", "else", "while", "for", "do", "break", "continue", "next",
           "nextfile", "exit", "return", "delete", "in", "getline", "print", "printf"}
_AWK_BUILTINS = {"length", "substr", "index", "split", "sub", "gsub", "match", "sprintf", "sin", "cos", "atan2",
                 "exp", "log", "sqrt", "int", "rand", "srand", "tolower", "toupper", "close", "system", "fflush"}
_AWK_OPS = ["**=", "+=", "-=", "*=", "/=", "%=", "^=", "==", "<=", ">=", "!=", "++", "--", "&&", "||", ">>", "!~",
            "**", "{", "}", "(", ")", "[", "]", ";", ",", "+", "-", "*", "/", "%", "^", "!", ">", "<", "|", "?", ":",
            "~", "$", "="]
_AWK_NUM = re.compile(r"(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def _awk_lex(src):
    toks, i, n = [], 0, len(src)

    def operand_before():
        if not toks:
            return False
        k, v = toks[-1]
        return k in ("num", "str", "ere", "name", "builtin") or (k == "op" and v in (")", "]", "++", "--"))

    while i < n:
        c = src[i]
        if c in " \t\r":
            i += 1
        elif c == "\\" and src.startswith("\n", i + 1):
            i += 2
        elif c == "\\" and src.startswith("\r\n", i + 1):
            i += 3
        elif c == "\n":
            toks.append(("nl", "\n"))
            i += 1
        elif c == "#":
            while i < n and src[i] != "\n":
                i += 1
        elif c == '"':
            j, buf = i + 1, []
            while j < n and src[j] != '"':
                if src[j] == "\\" and j + 1 < n:
                    buf.append(src[j:j + 2])
                    j += 2
                    continue
                if src[j] == "\n":
                    raise Fail("awk: a string isn't closed", 2)
                buf.append(src[j])
                j += 1
            if j >= n:
                raise Fail("awk: a string isn't closed", 2)
            toks.append(("str", unescape("".join(buf))))
            i = j + 1
        elif c == "/" and not operand_before():
            j, buf, inbr = i + 1, [], False
            while j < n:
                ch = src[j]
                if ch == "\\" and j + 1 < n:
                    buf.append("/" if src[j + 1] == "/" else src[j:j + 2])
                    j += 2
                    continue
                if ch == "\n":
                    raise Fail("awk: a /regex/ isn't closed", 2)
                if ch == "[" and not inbr:
                    inbr = True
                    buf.append(ch)
                    j += 1
                    for extra in "^]":
                        if j < n and src[j] == extra:
                            buf.append(extra)
                            j += 1
                    continue
                if ch == "]" and inbr:
                    inbr = False
                elif ch == "/" and not inbr:
                    break
                buf.append(ch)
                j += 1
            if j >= n:
                raise Fail("awk: a /regex/ isn't closed", 2)
            toks.append(("ere", compile_re("".join(buf))))
            i = j + 1
        elif c.isdigit() or (c == "." and i + 1 < n and src[i + 1].isdigit()):
            m = _AWK_NUM.match(src, i)
            toks.append(("num", float(m.group())))
            i = m.end()
        elif c.isalpha() or c == "_":
            m = re.compile(r"\w+").match(src, i)
            w = m.group()
            i = m.end()
            if w in _AWK_KW:
                toks.append(("kw", w))
            elif w in _AWK_BUILTINS:
                toks.append(("builtin", w))
            else:
                toks.append(("func" if src[i:i + 1] == "(" else "name", w))
        else:
            for op in _AWK_OPS:
                if src.startswith(op, i):
                    toks.append(("op", op))
                    i += len(op)
                    break
            else:
                raise Fail(f"awk: unexpected character {c!r}", 2)
    toks.append(("eof", ""))
    return toks


_ASSIGN_OPS = {"=", "+=", "-=", "*=", "/=", "%=", "^=", "**="}
_LVALUES = ("var", "field", "index")


class _AwkParser:
    def __init__(self, toks):
        self.t, self.p, self.ng = toks, 0, False

    def peek(self, k=0):
        return self.t[min(self.p + k, len(self.t) - 1)]

    def next(self):
        t = self.t[self.p]
        self.p = min(self.p + 1, len(self.t) - 1)
        return t

    def at(self, kind, value=None):
        k, v = self.peek()
        return k == kind and (value is None or v == value)

    def expect(self, kind, value=None):
        if not self.at(kind, value):
            self.error(f"expected {value or kind}")
        return self.next()

    def error(self, what="syntax error"):
        k, v = self.peek()
        near = "the end" if k == "eof" else "a new line" if k == "nl" else repr(v)
        raise Fail(f"awk: {what} near {near}", 2)

    def optnl(self):
        while self.at("nl"):
            self.next()

    def skip_terms(self):
        while self.at("nl") or self.at("op", ";"):
            self.next()

    def at_end(self):
        return self.at("nl") or self.at("op", ";") or self.at("op", "}") or self.at("eof")

    # program
    def program(self):
        begin, end, rules, funcs = [], [], [], {}
        self.skip_terms()
        while not self.at("eof"):
            if self.at("kw", "BEGIN"):
                self.next()
                self.optnl()
                begin.append(self.block())
            elif self.at("kw", "END"):
                self.next()
                self.optnl()
                end.append(self.block())
            elif self.at("kw", "function") or self.at("kw", "func"):
                self.next()
                k, name = self.next()
                if k not in ("name", "func"):
                    self.error("expected a function name")
                self.expect("op", "(")
                params = []
                while not self.at("op", ")"):
                    params.append(self.expect("name")[1])
                    if self.at("op", ","):
                        self.next()
                        self.optnl()
                self.expect("op", ")")
                self.optnl()
                body = self.block()
                funcs[name] = (params, body, _array_names(body) & set(params))
            else:
                pat = pat2 = None
                if not self.at("op", "{"):
                    pat = self.expr()
                    if self.at("op", ","):
                        self.next()
                        self.optnl()
                        pat2 = self.expr()
                action = self.block() if self.at("op", "{") else None
                rules.append((pat, pat2, action))
            self.skip_terms()
        return begin, end, rules, funcs

    def block(self):
        self.expect("op", "{")
        body = []
        while True:
            self.skip_terms()
            if self.at("op", "}"):
                self.next()
                return ("block", body)
            if self.at("eof"):
                self.error("a { isn't closed")
            body.append(self.stmt())

    def body(self):
        if self.at("op", ";"):
            self.next()
            return ("block", [])
        return self.stmt()

    def stmt(self):
        if self.at("op", "{"):
            return self.block()
        if self.at("kw", "if"):
            self.next()
            self.expect("op", "(")
            cond = self.expr()
            self.expect("op", ")")
            self.optnl()
            yes = self.body()
            save = self.p
            self.skip_terms()
            if self.at("kw", "else"):
                self.next()
                self.optnl()
                return ("if", cond, yes, self.body())
            self.p = save
            return ("if", cond, yes, None)
        if self.at("kw", "while"):
            self.next()
            self.expect("op", "(")
            cond = self.expr()
            self.expect("op", ")")
            if self.at("op", ";"):
                self.next()
                return ("while", cond, ("block", []))
            self.optnl()
            return ("while", cond, self.body())
        if self.at("kw", "do"):
            self.next()
            self.optnl()
            body = self.body()
            self.skip_terms()
            self.expect("kw", "while")
            self.expect("op", "(")
            cond = self.expr()
            self.expect("op", ")")
            self.end_simple()
            return ("do", body, cond)
        if self.at("kw", "for"):
            self.next()
            self.expect("op", "(")
            if (self.peek()[0] == "name" and self.peek(1) == ("kw", "in") and self.peek(2)[0] == "name"
                    and self.peek(3) == ("op", ")")):
                var = self.next()[1]
                self.next()
                arr = self.next()[1]
                self.next()
                self.optnl()
                return ("forin", var, arr, self.body())
            init = None if self.at("op", ";") else self.expr()
            self.expect("op", ";")
            self.optnl()
            cond = None if self.at("op", ";") else self.expr()
            self.expect("op", ";")
            self.optnl()
            step = None if self.at("op", ")") else self.expr()
            self.expect("op", ")")
            self.optnl()
            return ("for", init, cond, step, self.body())
        if self.at("op", ";"):
            self.next()
            return ("block", [])
        s = self.simple()
        self.end_simple()
        return s

    def end_simple(self):
        if self.at("op", ";") or self.at("nl"):
            self.skip_terms()
        elif not (self.at("op", "}") or self.at("eof")):
            self.error()

    def simple(self):
        k, v = self.peek()
        if k == "kw":
            if v in ("print", "printf"):
                return self.print_stmt(v)
            if v in ("next", "nextfile", "break", "continue"):
                self.next()
                return (v,)
            if v in ("exit", "return"):
                self.next()
                return (v, None if self.at_end() else self.expr())
            if v == "delete":
                self.next()
                name = self.expect("name")[1]
                if self.at("op", "["):
                    self.next()
                    idx = self.exprlist()
                    self.expect("op", "]")
                    return ("delete", name, idx)
                return ("delete", name, None)
            if v == "getline":
                raise Fail("awk: getline isn't supported here (awk reads its input by itself: use a main { } rule)", 2)
        return ("expr", self.expr())

    def print_stmt(self, kind):
        self.next()
        args = []
        saved, self.ng = self.ng, True
        if not (self.at_end() or self.at("op", ">") or self.at("op", ">>") or self.at("op", "|")):
            args = self.exprlist()
        if len(args) == 1 and args[0][0] == "group":
            args = args[0][1]
        redir = None
        if self.at("op", ">") or self.at("op", ">>") or self.at("op", "|"):
            op = self.next()[1]
            if op == "|":
                raise Fail("awk: print | \"command\" isn't supported here: pipe awk's output to the command instead", 2)
            redir = (op, self.concat())
        self.ng = saved
        if kind == "printf" and not args:
            self.error("printf needs a format")
        return (kind, args, redir)

    def exprlist(self):
        items = [self.expr()]
        while self.at("op", ","):
            self.next()
            self.optnl()
            items.append(self.expr())
        return items

    # expressions, lowest precedence first
    def expr(self):
        left = self.ternary()
        k, v = self.peek()
        if k == "op" and v in _ASSIGN_OPS and left[0] in _LVALUES:
            self.next()
            self.optnl()
            return ("assign", v, left, self.expr())
        return left

    def ternary(self):
        c = self.or_()
        if self.at("op", "?"):
            self.next()
            self.optnl()
            a = self.expr()
            self.optnl()
            self.expect("op", ":")
            self.optnl()
            return ("cond", c, a, self.expr())
        return c

    def or_(self):
        a = self.and_()
        while self.at("op", "||"):
            self.next()
            self.optnl()
            a = ("or", a, self.and_())
        return a

    def and_(self):
        a = self.in_()
        while self.at("op", "&&"):
            self.next()
            self.optnl()
            a = ("and", a, self.in_())
        return a

    def in_(self):
        a = self.match_()
        while self.at("kw", "in"):
            self.next()
            name = self.expect("name")[1]
            a = ("in", a[1] if a[0] == "group" else [a], name)
        return a

    def match_(self):
        a = self.rel()
        while self.at("op", "~") or self.at("op", "!~"):
            neg = self.next()[1] == "!~"
            a = ("match", neg, a, self.rel())
        return a

    def rel(self):
        a = self.concat()
        k, v = self.peek()
        if k == "op" and v in ("<", "<=", "!=", "==", ">=", ">") and not (v == ">" and self.ng):
            self.next()
            return ("cmp", v, a, self.concat())
        return a

    def concat(self):
        a = self.additive()
        while True:
            k, v = self.peek()
            if k in ("num", "str", "ere", "name", "func", "builtin") or (k == "op" and v in ("$", "(", "!", "++", "--")):
                a = ("concat", a, self.additive())
            else:
                return a

    def additive(self):
        a = self.mul()
        while self.at("op", "+") or self.at("op", "-"):
            a = ("bin", self.next()[1], a, self.mul())
        return a

    def mul(self):
        a = self.unary()
        while self.at("op", "*") or self.at("op", "/") or self.at("op", "%"):
            a = ("bin", self.next()[1], a, self.unary())
        return a

    def unary(self):
        if self.at("op", "!"):
            self.next()
            return ("not", self.unary())
        if self.at("op", "-"):
            self.next()
            return ("neg", self.unary())
        if self.at("op", "+"):
            self.next()
            return ("pos", self.unary())
        return self.power()

    def power(self):
        base = self.postfix()
        if self.at("op", "^") or self.at("op", "**"):
            self.next()
            return ("bin", "^", base, self.unary())
        return base

    def postfix(self):
        p = self.primary()
        if p[0] in _LVALUES and (self.at("op", "++") or self.at("op", "--")):
            return ("post", self.next()[1], p)
        return p

    def primary(self):
        k, v = self.next()
        if k == "num":
            return ("num", v)
        if k == "str":
            return ("str", v)
        if k == "ere":
            return ("re", v)
        if k == "op":
            if v == "$":
                if self.at("op", "++") or self.at("op", "--"):
                    op = self.next()[1]
                    return ("field", ("pre", op, self.primary()))
                if self.at("op", "-"):
                    self.next()
                    return ("field", ("neg", self.primary()))
                return ("field", self.primary())
            if v == "(":
                saved, self.ng = self.ng, False
                e = self.expr()
                if self.at("op", ","):
                    items = [e]
                    while self.at("op", ","):
                        self.next()
                        self.optnl()
                        items.append(self.expr())
                    self.expect("op", ")")
                    self.ng = saved
                    return ("group", items)
                self.expect("op", ")")
                self.ng = saved
                return ("paren", e)
            if v in ("++", "--"):
                lv = self.primary()
                if lv[0] not in _LVALUES:
                    self.error(f"{v} needs a variable")
                return ("pre", v, lv)
            if v == "-":
                return ("neg", self.unary())
            if v == "!":
                return ("not", self.unary())
        if k == "builtin":
            if self.at("op", "("):
                self.next()
                saved, self.ng = self.ng, False
                args = [] if self.at("op", ")") else self.exprlist()
                self.expect("op", ")")
                self.ng = saved
                return ("call", v, args)
            if v == "length":
                return ("call", "length", [])
            self.p -= 1
            self.error(f"{v} needs (…)")
        if k == "func":
            self.expect("op", "(")
            saved, self.ng = self.ng, False
            args = [] if self.at("op", ")") else self.exprlist()
            self.expect("op", ")")
            self.ng = saved
            return ("ucall", v, args)
        if k == "name":
            if self.at("op", "["):
                self.next()
                saved, self.ng = self.ng, False
                idx = self.exprlist()
                self.expect("op", "]")
                self.ng = saved
                return ("index", v, idx)
            return ("var", v)
        if k == "kw" and v == "getline":
            raise Fail("awk: getline isn't supported here (awk reads its input by itself: use a main { } rule)", 2)
        self.p -= 1
        self.error()


def _array_names(node):
    """Names a function body uses as arrays (so array arguments are passed by reference)."""
    found = set()

    def walk(x):
        if isinstance(x, tuple):
            if x and x[0] in ("index", "delete") and isinstance(x[1], str):
                found.add(x[1])
            elif x and x[0] == "in":
                found.add(x[2])
            elif x and x[0] == "forin":
                found.add(x[2])
            elif x and x[0] == "call" and x[1] in ("split", "match"):
                pos = 1 if x[1] == "split" else 2
                if len(x[2]) > pos and x[2][pos][0] == "var":
                    found.add(x[2][pos][1])
            for y in x:
                walk(y)
        elif isinstance(x, list):
            for y in x:
                walk(y)

    walk(node)
    return found


class _AwkNext(Exception):
    pass


class _AwkNextFile(Exception):
    pass


class _AwkExit(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


class _AwkReturn(Exception):
    def __init__(self, value):
        super().__init__()
        self.value = value


class _AwkBreak(Exception):
    pass


class _AwkContinue(Exception):
    pass


class _Awk:
    def __init__(self, prog, stdin):
        self.begin, self.end, self.rules, self.funcs = prog
        self.g = {"FS": " ", "OFS": " ", "ORS": "\n", "RS": "\n", "NR": 0.0, "FNR": 0.0, "SUBSEP": "\x1c",
                  "FILENAME": "", "RSTART": 0.0, "RLENGTH": -1.0, "CONVFMT": "%.6g", "OFMT": "%.6g",
                  "ENVIRON": {}}
        self.locals = None
        self.depth = 0
        self.record, self.fields, self.split_done = "", [], True
        self.out, self.err, self.files = [], [], {}
        self.ranges = [False] * len(self.rules)
        self.stdin = stdin
        self.seed = 0.0
        self.rng = random.Random(0)

    # variables
    def raw(self, name):
        if self.locals is not None and name in self.locals:
            return self.locals[name]
        return self.g.get(name, UNSET)

    def get(self, name):
        if name == "NF" and not (self.locals is not None and name in self.locals):
            return float(len(self.split_fields()))
        v = self.raw(name)
        if isinstance(v, dict):
            raise Fail(f"awk: {name} is an array: use {name}[key]", 2)
        return v

    def set(self, name, v):
        if self.locals is not None and name in self.locals:
            if isinstance(self.locals[name], dict):
                raise Fail(f"awk: {name} is an array", 2)
            self.locals[name] = v
        elif name == "NF":
            self.set_nf(int(a_num(v)))
        else:
            if isinstance(self.g.get(name), dict):
                raise Fail(f"awk: {name} is an array", 2)
            self.g[name] = v
        return v

    def array(self, name):
        scope = self.locals if self.locals is not None and name in self.locals else self.g
        v = scope.get(name, UNSET)
        if isinstance(v, dict):
            return v
        if isinstance(v, _Uninit):
            d = scope[name] = {}
            return d
        raise Fail(f"awk: {name} isn't an array", 2)

    def gs(self, name):
        return a_str(self.g.get(name, ""))

    def tostr(self, v):
        return a_str(v, self.gs("CONVFMT"))

    # fields
    def split(self, s, fs=None, rx=None):
        if rx is None:
            if fs == " ":
                return s.split()
            if fs == "":
                return list(s)
            if len(fs) == 1 and fs != "\\" and self.gs("RS") != "":
                return s.split(fs) if s else []
            if len(fs) == 1:
                fs = re.escape(fs)
            rx = compile_re(fs if self.gs("RS") != "" else f"(?:{fs})|\n")
        if not s:
            return []
        pieces, last = [], 0
        for m in rx.finditer(s):
            if m.end() == m.start():
                continue
            pieces.append(s[last:m.start()])
            last = m.end()
        pieces.append(s[last:])
        return pieces

    def split_fields(self):
        if not self.split_done:
            self.fields = [_StrNum(f) for f in self.split(self.record, self.gs("FS"))]
            self.split_done = True
        return self.fields

    def field(self, i):
        if i == 0:
            return _StrNum(self.record)
        if i < 0:
            raise Fail(f"awk: there's no field ${i}", 2)
        f = self.split_fields()
        return f[i - 1] if i <= len(f) else UNSET

    def set_field(self, i, v):
        if i == 0:
            self.record, self.split_done = self.tostr(v), False
            return
        if i < 0:
            raise Fail(f"awk: there's no field ${i}", 2)
        f = self.split_fields()
        while len(f) < i:
            f.append(_StrNum(""))
        f[i - 1] = _StrNum(self.tostr(v))
        self.record = self.gs("OFS").join(f)

    def set_nf(self, n):
        f = self.split_fields()
        del f[max(n, 0):]
        while len(f) < n:
            f.append(_StrNum(""))
        self.record = self.gs("OFS").join(f)

    # expressions
    def ev(self, e):
        k = e[0]
        if k == "num" or k == "str":
            return e[1]
        if k == "field":
            return self.field(int(a_num(self.ev(e[1]))))
        if k == "var":
            return self.get(e[1])
        if k == "paren":
            return self.ev(e[1])
        if k == "concat":
            return self.tostr(self.ev(e[1])) + self.tostr(self.ev(e[2]))
        if k == "bin":
            return self.arith(e[1], a_num(self.ev(e[2])), a_num(self.ev(e[3])))
        if k == "cmp":
            a, b = self.ev(e[2]), self.ev(e[3])
            if a_isnum(a) and a_isnum(b):
                x, y = a_num(a), a_num(b)
            else:
                x, y = self.tostr(a), self.tostr(b)
            op = e[1]
            r = (x < y if op == "<" else x <= y if op == "<=" else x == y if op == "==" else x != y if op == "!="
                 else x >= y if op == ">=" else x > y)
            return 1.0 if r else 0.0
        if k == "index":
            arr, key = self.array(e[1]), self.key(e[2])
            v = arr.get(key)
            if v is None:
                v = arr[key] = UNSET
            return v
        if k == "assign":
            return self.assign(e)
        if k == "and":
            return 1.0 if a_bool(self.ev(e[1])) and a_bool(self.ev(e[2])) else 0.0
        if k == "or":
            return 1.0 if a_bool(self.ev(e[1])) or a_bool(self.ev(e[2])) else 0.0
        if k == "not":
            return 0.0 if a_bool(self.ev(e[1])) else 1.0
        if k == "neg":
            return -a_num(self.ev(e[1]))
        if k == "pos":
            return a_num(self.ev(e[1]))
        if k == "cond":
            return self.ev(e[2]) if a_bool(self.ev(e[1])) else self.ev(e[3])
        if k == "match":
            hit = self.regex(e[3]).search(self.tostr(self.ev(e[2]))) is not None
            return 1.0 if hit != e[1] else 0.0
        if k == "re":
            return 1.0 if e[1].search(self.record) else 0.0
        if k == "in":
            return 1.0 if self.key(e[1]) in self.array(e[2]) else 0.0
        if k == "pre":
            v = a_num(self.ev(e[2])) + (1 if e[1] == "++" else -1)
            return self.store(e[2], v)
        if k == "post":
            old = a_num(self.ev(e[2]))
            self.store(e[2], old + (1 if e[1] == "++" else -1))
            return old
        if k == "call":
            return self.builtin(e[1], e[2])
        if k == "ucall":
            return self.call(e[1], e[2])
        if k == "group":
            raise Fail("awk: (a, b) only works before in, like  (i, j) in arr", 2)
        raise Fail(f"awk: can't evaluate {k}", 2)

    def key(self, exprs):
        return self.gs("SUBSEP").join(self.tostr(self.ev(x)) for x in exprs)

    def store(self, lv, v):
        k = lv[0]
        if k == "var":
            self.set(lv[1], v)
        elif k == "field":
            self.set_field(int(a_num(self.ev(lv[1]))), v)
        elif k == "index":
            self.array(lv[1])[self.key(lv[2])] = v
        else:
            raise Fail("awk: can only assign to a variable, a field or an array element", 2)
        return v

    def assign(self, e):
        op, lv, rhs = e[1], e[2], e[3]
        if op == "=":
            return self.store(lv, self.ev(rhs))
        r = a_num(self.ev(rhs))
        cur = a_num(self.ev(lv))
        return self.store(lv, self.arith("^" if op in ("^=", "**=") else op[0], cur, r))

    def arith(self, op, a, b):
        if op == "+":
            return a + b
        if op == "-":
            return a - b
        if op == "*":
            return a * b
        if op == "/":
            if b == 0:
                raise Fail("awk: division by zero", 2)
            return a / b
        if op == "%":
            if b == 0:
                raise Fail("awk: division by zero in %", 2)
            return math.fmod(a, b)
        try:
            r = a ** b
            return r if isinstance(r, float) else math.nan
        except OverflowError:
            return math.inf
        except ZeroDivisionError:
            return math.inf

    def regex(self, e):
        if e[0] == "re":
            return e[1]
        return compile_re(self.tostr(self.ev(e)))

    def sprintf(self, vals):
        def num(v):
            return 0.0 if v is None else a_num(v)

        def text(v):
            return "" if v is None else self.tostr(v)

        def char(v):
            if v is None:
                return ""
            if isinstance(v, float):
                return chr(int(v)) if 0 <= v < 0x110000 else ""
            return v[:1]

        return format_values(self.tostr(vals[0]), vals[1:], num, text, char)[0]

    def builtin(self, name, args):
        n = len(args)
        ev = self.ev
        if name == "length":
            if n == 0:
                return float(len(self.record))
            if args[0][0] == "var" and isinstance(self.raw(args[0][1]), dict):
                return float(len(self.raw(args[0][1])))
            return float(len(self.tostr(ev(args[0]))))
        if name == "substr":
            if n < 2:
                raise Fail("awk: substr(s, start[, length])", 2)
            s = self.tostr(ev(args[0]))
            start = round(a_num(ev(args[1])))
            end = start + round(a_num(ev(args[2]))) if n > 2 else len(s) + 1
            start, end = max(start, 1), min(end, len(s) + 1)
            return s[start - 1:end - 1] if end > start else ""
        if name == "index":
            return float(self.tostr(ev(args[0])).find(self.tostr(ev(args[1]))) + 1)
        if name == "split":
            if n < 2 or args[1][0] != "var":
                raise Fail("awk: split(s, array[, separator])", 2)
            s = self.tostr(ev(args[0]))
            if n > 2 and args[2][0] == "re":
                pieces = self.split(s, rx=args[2][1])
            else:
                pieces = self.split(s, self.tostr(ev(args[2])) if n > 2 else self.gs("FS"))
            arr = self.array(args[1][1])
            arr.clear()
            for i, p in enumerate(pieces, 1):
                arr[str(i)] = _StrNum(p)
            return float(len(pieces))
        if name in ("sub", "gsub"):
            if n < 2:
                raise Fail(f"awk: {name}(regex, replacement[, target])", 2)
            rx = self.regex(args[0])
            repl = self.tostr(ev(args[1]))
            target = args[2] if n > 2 else ("field", ("num", 0.0))
            if target[0] not in _LVALUES:
                raise Fail(f"awk: {name}'s third argument must be a variable or field", 2)
            s = self.tostr(ev(target))
            count = 0

            def rep(m):
                nonlocal count
                count += 1
                out, i = [], 0
                while i < len(repl):
                    c = repl[i]
                    if c == "\\" and i + 1 < len(repl) and repl[i + 1] in "&\\":
                        out.append(repl[i + 1])
                        i += 2
                        continue
                    out.append(m.group(0) if c == "&" else c)
                    i += 1
                return "".join(out)

            new = rx.sub(rep, s, count=0 if name == "gsub" else 1)
            if count:
                self.store(target, new)
            return float(count)
        if name == "match":
            s = self.tostr(ev(args[0]))
            m = self.regex(args[1]).search(s)
            self.g["RSTART"], self.g["RLENGTH"] = (float(m.start() + 1), float(len(m.group(0)))) if m else (0.0, -1.0)
            if n > 2 and args[2][0] == "var":
                arr = self.array(args[2][1])
                arr.clear()
                if m:
                    arr["0"] = _StrNum(m.group(0))
                    for i, grp in enumerate(m.groups(), 1):
                        if grp is not None:
                            arr[str(i)] = _StrNum(grp)
            return self.g["RSTART"]
        if name == "sprintf":
            if not args:
                raise Fail("awk: sprintf needs a format", 2)
            return self.sprintf([ev(a) for a in args])
        if name in ("sin", "cos", "exp", "log", "sqrt", "int"):
            x = a_num(ev(args[0])) if args else a_num(self.field(0))
            try:
                if name == "int":
                    return float(math.trunc(x))
                if name == "log":
                    return math.log(x) if x > 0 else (-math.inf if x == 0 else math.nan)
                if name == "sqrt":
                    return math.sqrt(x) if x >= 0 else math.nan
                return getattr(math, name)(x)
            except (OverflowError, ValueError):
                return math.inf if name == "exp" else math.nan
        if name == "atan2":
            return math.atan2(a_num(ev(args[0])), a_num(ev(args[1])))
        if name == "rand":
            return self.rng.random()
        if name == "srand":
            prev = self.seed
            self.seed = a_num(ev(args[0])) if args else float(int(time.time()))
            self.rng.seed(self.seed)
            return prev
        if name in ("tolower", "toupper"):
            s = self.tostr(ev(args[0]))
            return s.lower() if name == "tolower" else s.upper()
        if name == "close":
            f = self.files.pop(self.tostr(ev(args[0])), None)
            if f:
                f.close()
                return 0.0
            return -1.0
        if name == "fflush":
            return 0.0
        if name == "system":
            raise Fail("awk: system() isn't available here: run commands with the shell instead", 2)
        raise Fail(f"awk: unknown function {name}", 2)

    def call(self, name, args):
        fn = self.funcs.get(name)
        if fn is None:
            raise Fail(f"awk: function {name} isn't defined", 2)
        params, body, arrays = fn
        if len(args) > len(params):
            raise Fail(f"awk: {name} takes at most {len(params)} arguments", 2)
        frame = {}
        for i, p in enumerate(params):
            if i >= len(args):
                frame[p] = UNSET
                continue
            a = args[i]
            if a[0] == "var" and (isinstance(self.raw(a[1]), dict) or (p in arrays and isinstance(self.raw(a[1]), _Uninit))):
                frame[p] = self.array(a[1])
            else:
                frame[p] = self.ev(a)
        if self.depth > 300:
            raise Fail("awk: too many nested function calls", 2)
        saved, self.locals = self.locals, frame
        self.depth += 1
        try:
            self.run(body)
            return UNSET
        except _AwkReturn as r:
            return r.value
        finally:
            self.locals = saved
            self.depth -= 1

    # statements
    def run(self, s):
        k = s[0]
        if k == "block":
            for x in s[1]:
                self.run(x)
        elif k == "expr":
            self.ev(s[1])
        elif k in ("print", "printf"):
            if k == "print":
                text = (self.gs("OFS").join(a_str(self.ev(a), self.gs("OFMT")) for a in s[1])
                        if s[1] else self.record) + self.gs("ORS")
            else:
                text = self.sprintf([self.ev(a) for a in s[1]])
            self.emit(text, s[2])
        elif k == "if":
            if a_bool(self.ev(s[1])):
                self.run(s[2])
            elif s[3] is not None:
                self.run(s[3])
        elif k == "while":
            while a_bool(self.ev(s[1])):
                try:
                    self.run(s[2])
                except _AwkBreak:
                    break
                except _AwkContinue:
                    continue
        elif k == "do":
            while True:
                try:
                    self.run(s[1])
                except _AwkBreak:
                    break
                except _AwkContinue:
                    pass
                if not a_bool(self.ev(s[2])):
                    break
        elif k == "for":
            if s[1] is not None:
                self.ev(s[1])
            while s[2] is None or a_bool(self.ev(s[2])):
                try:
                    self.run(s[4])
                except _AwkBreak:
                    break
                except _AwkContinue:
                    pass
                if s[3] is not None:
                    self.ev(s[3])
        elif k == "forin":
            arr = self.array(s[2])
            for key in list(arr):
                if key not in arr:
                    continue
                self.store(("var", s[1]), _StrNum(key))
                try:
                    self.run(s[3])
                except _AwkBreak:
                    break
                except _AwkContinue:
                    pass
        elif k == "next":
            raise _AwkNext()
        elif k == "nextfile":
            raise _AwkNextFile()
        elif k == "exit":
            raise _AwkExit(int(a_num(self.ev(s[1]))) if s[1] is not None else 0)
        elif k == "return":
            raise _AwkReturn(self.ev(s[1]) if s[1] is not None else UNSET)
        elif k == "break":
            raise _AwkBreak()
        elif k == "continue":
            raise _AwkContinue()
        elif k == "delete":
            arr = self.array(s[1])
            if s[2] is None:
                arr.clear()
            else:
                arr.pop(self.key(s[2]), None)

    def emit(self, text, redir):
        if redir is None:
            self.out.append(text)
            return
        op, target = redir
        name = self.tostr(self.ev(target))
        if name in ("/dev/stdout", "-"):
            self.out.append(text)
        elif name == "/dev/stderr":
            self.err.append(text)
        else:
            f = self.files.get(name)
            if f is None:
                f = self.files[name] = open(resolve(name), "a" if op == ">>" else "w", encoding="utf-8")
            f.write(text)

    def records(self, text):
        rs = self.gs("RS")
        if rs == "":
            text = text.strip("\n")
            return re.split(r"\n\n+", text) if text else []
        if len(rs) == 1:
            recs = text.split(rs)
        else:
            recs = compile_re(rs).split(text)
        if recs and recs[-1] in ("", "\n"):
            recs.pop()
        return recs

    def matches(self, i, rule):
        pat, pat2, _ = rule
        if pat is None:
            return True
        if pat2 is None:
            return a_bool(self.ev(pat))
        if self.ranges[i]:
            if a_bool(self.ev(pat2)):
                self.ranges[i] = False
            return True
        if a_bool(self.ev(pat)):
            self.ranges[i] = not a_bool(self.ev(pat2))
            return True
        return False

    def main(self, operands):
        code = 0
        try:
            try:
                for b in self.begin:
                    self.run(b)
                if self.rules or self.end:
                    is_assign = re.compile(r"[A-Za-z_]\w*=").match
                    seq = operands if any(not is_assign(o) for o in operands) else operands + ["-"]
                    for o in seq:
                        if o != "-" and is_assign(o):
                            name, _, val = o.partition("=")
                            self.g[name] = _StrNum(unescape(val))
                            continue
                        text = self.stdin if o == "-" else read_text(o)
                        self.g["FILENAME"], self.g["FNR"] = ("" if o == "-" else o), 0.0
                        try:
                            for rec in self.records(text):
                                self.g["NR"] = a_num(self.g["NR"]) + 1
                                self.g["FNR"] = a_num(self.g["FNR"]) + 1
                                self.record, self.split_done = rec, False
                                try:
                                    for i, rule in enumerate(self.rules):
                                        if self.matches(i, rule):
                                            if rule[2] is None:
                                                self.out.append(self.record + self.gs("ORS"))
                                            else:
                                                self.run(rule[2])
                                except _AwkNext:
                                    pass
                        except _AwkNextFile:
                            pass
            except _AwkExit as e:
                code = e.code
            try:
                for b in self.end:
                    self.run(b)
            except _AwkExit as e:
                code = e.code
        except (_AwkNext, _AwkNextFile):
            raise Fail("awk: next can't be used in BEGIN or END", 2)
        except (_AwkBreak, _AwkContinue):
            raise Fail("awk: break or continue outside a loop", 2)
        except _AwkReturn:
            raise Fail("awk: return outside a function", 2)
        except RecursionError:
            raise Fail("awk: the program nests too deeply", 2)
        finally:
            for f in self.files.values():
                f.close()
        return code


def c_awk(args, stdin):
    fs, assigns, prog, i = None, [], None, 0
    while i < len(args):
        a = args[i]
        if a == "--":
            i += 1
            break
        if a.startswith("-F"):
            if len(a) > 2:
                fs = a[2:]
            else:
                i += 1
                fs = args[i] if i < len(args) else None
        elif a.startswith("-v"):
            if len(a) > 2:
                assigns.append(a[2:])
            else:
                i += 1
                assigns.append(args[i] if i < len(args) else "")
        elif a == "-f":
            i += 1
            prog = read_text(args[i])
        elif a in ("--posix", "--traditional", "-P", "-c"):
            pass
        elif a.startswith("-") and a != "-":
            raise Fail(f"unknown option {a} (awk knows -F, -v and -f)", 2)
        else:
            break
        i += 1
    rest = args[i:]
    if prog is None:
        if not rest:
            raise Fail("usage: awk [-F sep] [-v name=value] 'program' [files]", 2)
        prog, rest = rest[0], rest[1:]
    awk = _Awk(_AwkParser(_awk_lex(prog)).program(), stdin)
    if fs is not None:
        awk.g["FS"] = "\t" if fs == "t" else unescape(fs)
    for spec in assigns:
        name, eq, val = spec.partition("=")
        if not eq or not re.fullmatch(r"[A-Za-z_]\w*", name):
            raise Fail(f"-v {spec}: use -v name=value", 2)
        awk.g[name] = _StrNum(unescape(val))
    code = awk.main(rest)
    return _Result("".join(awk.out), "".join(awk.err), code)


# ------------------------------------------------------------------- help

HELP = {
    "awk": "awk [-F sep] [-v name=value] 'program' [files]: per-line programs: $1…$NF, NR, NF, FS/OFS, "
           "BEGIN/END, /regex/ and expression patterns, arrays, if/for/while, print, printf, length, substr, "
           "index, split, sub, gsub, match, sprintf, tolower, toupper, int; print > \"file\". "
           "e.g. awk -F, '$3 > 100 {n++; s += $3} END {print n, s/n}' data.csv",
    "base64": "base64 [-d] [-w cols] [file]: encode or decode",
    "basename": "basename path [suffix]: the last part of a path",
    "bash": "bash -c \"command line\"  or  bash script.sh: runs it in this same shell (sh works too)",
    "cat": "cat [-n] [-b] [-s] [-A] [files]: print files (or stdin)",
    "cd": "cd folder: changes folder for the rest of this command line only",
    "chmod": "chmod …: accepted, does nothing (files here have no permissions to change)",
    "column": "column -t [-s sep] [-o outsep]: align columns into a table",
    "command": "command -v name: prints the name if this shell has it",
    "cp": "cp [-r] source… target",
    "cut": "cut -d sep -f 1,3-5  or  cut -c 1-10",
    "date": "date [-u] [+FORMAT]: e.g. date +%Y-%m-%d",
    "diff": "diff [-u] file1 file2: unified diff, exit 1 if they differ",
    "dirname": "dirname path: the folder part of a path",
    "du": "du [-s] [-h] [-a] [-c] [-d depth] [paths]: sizes",
    "echo": "echo [-n] [-e] text",
    "exit": "exit [code]: stop the command line",
    "false": "false: exit 1",
    "find": "find [paths] [tests] [actions]: -name/-iname 'glob', -path, -regex, -type f|d, -maxdepth/-mindepth N, "
            "-size +10k, -mtime -1, -mmin -30, -newer file, -empty, ! / -not, -o, ( ), -print, -print0, "
            "-exec cmd {} \\; or -exec cmd {} +, -delete",
    "grep": "grep [-inrvclLowxqEFPHh] [-A/-B/-C n] [-m n] [-e pattern] [--include=glob] [--exclude=glob] "
            "pattern [files]: basic regex by default, -E extended, -F fixed text, -P Python regex",
    "head": "head [-n N] [-c bytes] [files]",
    "help": "help [command]: this list, or one command's usage",
    "ls": "ls [-laR1hAF] [paths]",
    "md5sum": "md5sum [files]  (also sha1sum, sha256sum, sha512sum)",
    "mkdir": "mkdir [-p] folders",
    "rmdir": "rmdir folders: remove empty folders",
    "mv": "mv source… target",
    "nl": "nl [-b a] [-w width] [-s sep] [files]: number lines",
    "paste": "paste [-d delims] [-s] files: join lines side by side",
    "printf": "printf format [args…]: %s %d %f %x %5.2f %-10s %b, \\n \\t; the format repeats for extra args",
    "pwd": "pwd",
    "python": "python file.py [args] | python -c \"code\" | python -m module | python (script on stdin, e.g. a "
              "heredoc): Python 3 with its standard library, in this same sandbox",
    "realpath": "realpath path: the full path (inside the workspace)",
    "rev": "rev [files]: reverse each line",
    "rm": "rm [-rf] paths (never the workspace itself)",
    "sed": "sed [-n] [-E] [-i[suffix]] [-e script] 'script' [files]: s/re/new/[g][p][i][N], d, p, q, a/i/c text, "
           "y/abc/xyz/, =, n/N/D/P, h/H/g/G/x, b/t labels, { }; addresses N, $, /re/, N,M, /a/,/b/, N~step, !. "
           "Basic regex by default (\\( \\) \\{ \\} \\+ \\?), -E for extended; \\1 and & in replacements. "
           "e.g. sed -i 's/old/new/g' file.txt   sed -n '10,20p' file   sed '/^#/d'",
    "seq": "seq [-s sep] [-w] [first [step]] last",
    "set": "set -e | set -o pipefail: stop on the first failure | fail a pipeline if any part fails",
    "sleep": "sleep seconds (60 at most)",
    "sort": "sort [-rnufhVsb] [-t sep] [-k N[,M][n|r]] [-o file] [files]",
    "tac": "tac [files]: lines in reverse order",
    "tail": "tail [-n N] [-n +N] [files]",
    "tee": "tee [-a] files: copy stdin to files and to the output",
    "test": "test EXPR  or  [ EXPR ]: -e/-f/-d/-s file, -n/-z text, a = b, a != b, n -eq/-lt/-gt… m, !, -a, -o",
    "touch": "touch files",
    "tr": "tr [-d] [-s] [-c] set1 [set2]: a-z, [:upper:], [:lower:], [:digit:], [:space:], \\n, \\t",
    "tree": "tree [-L depth] [-a] [-d] [path]",
    "true": "true: exit 0",
    "type": "type name: says whether this shell has it",
    "uniq": "uniq [-c] [-d] [-u] [-i] [input [output]]",
    "wc": "wc [-l] [-w] [-c] [-m] [files]",
    "which": "which name: says whether this shell has it",
    "xargs": "xargs [-n N] [-I {}] [-0] [-d delim] [-t] command [args]: runs a command of this shell with "
             "stdin's items as arguments, e.g. find . -name '*.log' | xargs rm",
}
for _alias, _of in (("python3", "python"), ("sh", "bash"), ("[", "test"), ("[[", "test"), ("egrep", "grep"),
                    ("fgrep", "grep"), ("sha1sum", "md5sum"), ("sha256sum", "md5sum"), ("sha512sum", "md5sum"),
                    ("readlink", "realpath")):
    HELP.setdefault(_alias, HELP[_of])

SYNTAX = (
    "Syntax: pipes |, && and ||, ; or new lines between commands, redirects > >> < 2> 2>&1 2>/dev/null &> >&2, "
    "heredocs (cat > file <<'EOF' … EOF), here-strings (<<< text), 'single' and \"double\" quotes, globs "
    "(* ? [ ] **), braces ({a,b} {1..5}), # comments, \\ at the end of a line. Not here: variables, $(…), "
    "backticks, for/while/if, functions, ( ) subshells, & background jobs: use pipes and xargs, awk, or python."
)


def c_help(args, stdin):
    if args:
        out = [HELP.get(a, f"{a}: not a command of this shell") for a in args]
        return joined(out)
    names = sorted(n for n in COMMANDS if n not in ("[[",))
    return (
        "This is Chummy's own small shell, inside the sandbox: the workspace only, no network, nothing to "
        "install. Commands:\n"
        + "\n".join("  " + " ".join(names[i:i + 12]) for i in range(0, len(names), 12))
        + "\n" + SYNTAX + "\nhelp COMMAND shows a command's options (or COMMAND --help).\n"
    )


NETWORK = "there's no network in this shell: use the fetch_page tool to read a web page (if you have it)"
NOT_HERE = {
    "curl": NETWORK, "wget": NETWORK, "ssh": NETWORK, "scp": NETWORK, "ping": NETWORK, "nc": NETWORK,
    "sudo": "there's no sudo here, and nothing to install: everything runs inside the workspace",
    "apt": "nothing can be installed here", "apt-get": "nothing can be installed here", "brew": "nothing can be installed here",
    "npm": "there's no Node.js here: only Python 3 with its standard library",
    "node": "there's no Node.js here: only Python 3 with its standard library",
    "npx": "there's no Node.js here: only Python 3 with its standard library",
    "jq": "there's no jq here: use python, e.g.  python -c \"import json,sys; print(json.load(sys.stdin)['key'])\" < f.json",
    "git": "there's no git here",
    "make": "there's no make or compiler here: only Python 3",
    "gcc": "there's no compiler here: only Python 3", "cc": "there's no compiler here: only Python 3",
    "vim": "no interactive programs: view with cat/head/sed -n '5,20p', edit with sed -i or a heredoc",
    "vi": "no interactive programs: view with cat/head/sed -n '5,20p', edit with sed -i or a heredoc",
    "nano": "no interactive programs: view with cat/head/sed -n '5,20p', edit with sed -i or a heredoc",
    "less": "no interactive programs: use cat, head, tail or sed -n '5,20p'",
    "more": "no interactive programs: use cat, head, tail or sed -n '5,20p'",
    "top": "no interactive programs here", "ps": "there are no processes to list here",
    "kill": "there are no processes here",
    "export": NO_VARS, "for": NO_VARS, "while": NO_VARS, "if": NO_VARS, "until": NO_VARS, "case": NO_VARS,
    "function": NO_VARS, "do": NO_VARS, "done": NO_VARS, "then": NO_VARS, "fi": NO_VARS, "local": NO_VARS,
    "declare": NO_VARS, "read": NO_VARS, "source": NO_VARS, ".": NO_VARS, "alias": NO_VARS, "eval": NO_VARS,
    "ln": "links can't be made here (they could point outside the workspace)",
}


def c_rmdir(args, stdin):
    o, paths = flags(args, "p")
    for p in paths:
        full = resolve(p)
        if full == chummy.WORKSPACE:
            raise Fail("refusing to remove the workspace")
        try:
            os.rmdir(full)
        except FileNotFoundError:
            raise Fail(f"{p}: no such folder")
        except OSError:
            raise Fail(f"{p}: folder isn't empty")
    return ""


COMMANDS = {
    "echo": c_echo, "printf": c_printf, "cat": c_cat, "head": c_head, "tail": c_tail, "wc": c_wc,
    "grep": c_grep, "egrep": lambda a, s: c_grep(a, s, extended=True), "fgrep": lambda a, s: c_grep(a, s, fixed=True),
    "sed": c_sed, "awk": c_awk, "gawk": c_awk, "nawk": c_awk, "mawk": c_awk,
    "ls": c_ls, "find": c_find, "tree": c_tree, "du": c_du, "mkdir": c_mkdir, "rm": c_rm, "rmdir": c_rmdir,
    "cp": c_cp, "mv": c_mv, "touch": c_touch,
    "sort": c_sort, "uniq": c_uniq, "cut": c_cut, "tr": c_tr, "paste": c_paste, "column": c_column, "nl": c_nl,
    "tac": c_tac, "rev": c_rev, "tee": c_tee, "seq": c_seq, "diff": c_diff, "xargs": c_xargs,
    "basename": c_basename, "dirname": c_dirname, "realpath": c_realpath, "readlink": c_realpath,
    "md5sum": _hashsum("md5"), "sha1sum": _hashsum("sha1"), "sha256sum": _hashsum("sha256"),
    "sha512sum": _hashsum("sha512"), "base64": c_base64,
    "pwd": c_pwd, "cd": c_cd, "date": c_date, "sleep": c_sleep, "true": c_true, "false": c_false,
    ":": c_true, "test": c_test, "[": c_bracket, "[[": lambda a, s: c_bracket(a, s, "]]"),
    "which": c_which, "type": c_type, "command": c_command, "help": c_help, "set": c_set, "exit": c_exit,
    "chmod": c_chmod,
    "python": c_python, "python3": c_python, "pip": c_pip, "pip3": c_pip, "bash": c_shell, "sh": c_shell,
}


def invoke(words, stdin):
    """Runs one command (no redirects) of this shell. Returns (out, err, code)."""
    name, args = words[0], words[1:]
    if args == ["--help"] and name in HELP:
        return HELP[name] + "\n", "", 0
    fn = COMMANDS.get(name)
    if fn is None:
        if re.fullmatch(r"[A-Za-z_]\w*=.*", name):
            return "", f"{name.split('=')[0]}=…: {NO_VARS}\n", 2
        if "/" in name:
            try:
                full = resolve(name)
            except Fail as e:
                return "", f"{e}\n", 126
            if os.path.isfile(full):
                with open(full, encoding="utf-8", errors="replace") as f:
                    first = f.readline()
                if name.endswith(".py") or (first.startswith("#!") and "python" in first):
                    fn, args = c_python, [name] + args
                elif name.endswith(".sh") or (first.startswith("#!") and "sh" in first):
                    fn, args = c_shell, [name]
                else:
                    return "", f"{name}: only .py and .sh scripts can be run here\n", 126
            else:
                return "", f"{name}: no such file\n", 127
        else:
            why = NOT_HERE.get(name)
            if why:
                return "", f"{name}: {why}\n", 127
            return "", (
                f"{name}: command not found. This is Chummy's own small shell, not a real one: run  help  "
                f"to see its commands. For anything else, write a python script.\n"
            ), 127
    try:
        r = fn(args, stdin)
    except Fail as e:
        msg = str(e)
        return "", (msg if msg.startswith(name + ": ") else f"{name}: {msg}" if msg else "") + ("\n" if msg else ""), e.code
    except (OSError, ValueError, IndexError, re.error) as e:
        return "", f"{name}: {e}\n", 1
    if isinstance(r, _Result):
        return r.out, r.err, r.code
    if isinstance(r, _Diff):
        return str(r), "", 1
    return r, "", 0


# ------------------------------------------------------------------ parser

class Word(str):
    """A word of the command line; `glob` is false if any of it was quoted."""

    glob = True


class Op(str):
    """An operator: ; && || | & < > >> << <<< or a redirect of stderr."""


# Redirects of stderr, at the start of a word.
_REDIRS = [
    (re.compile(r"2>&1"), ["__ERR_TO_OUT__"]),
    (re.compile(r"1?>&2"), ["__OUT_TO_ERR__"]),
    (re.compile(r"&>>"), ["__ERR_TO_OUT__", ">>"]),
    (re.compile(r"&>"), ["__ERR_TO_OUT__", ">"]),
    (re.compile(r"2>\s*/dev/null(?![\w/.])"), ["__ERR_NULL__"]),
    (re.compile(r"2>>"), ["__ERR_APPEND__"]),
    (re.compile(r"2>"), ["__ERR_FILE__"]),
]
_HEREDOC = re.compile(r"<<(-?)[ \t]*(?:'([^'\n]*)'|\"([^\"\n]*)\"|\\?([^\s;&|<>()]+))")


def _braces(w):
    """{a,b} and {1..5} in an unquoted word."""
    m = re.search(r"\{(-?\d+)\.\.(-?\d+)\}", w)
    if m:
        lo, hi = int(m.group(1)), int(m.group(2))
        step = 1 if hi >= lo else -1
        if abs(hi - lo) > 10000:
            return [w]
        return [x for k in range(lo, hi + step, step) for x in _braces(w[:m.start()] + str(k) + w[m.end():])]
    depth, start = 0, -1
    for i, c in enumerate(w):
        if c == "{":
            if depth == 0:
                start = i
            depth += 1
        elif c == "}" and depth:
            depth -= 1
            if depth == 0:
                inner = w[start + 1:i]
                parts, d, cur = [], 0, []
                for ch in inner:
                    if ch == "," and d == 0:
                        parts.append("".join(cur))
                        cur = []
                        continue
                    d += ch == "{"
                    d -= ch == "}"
                    cur.append(ch)
                parts.append("".join(cur))
                if len(parts) < 2:
                    continue
                return [x for p in parts for x in _braces(w[:start] + p + w[i + 1:])]
    return [w]


def tokens(text):
    """Splits a command line (several lines allowed) into words and operators."""
    toks, word, quoted, in_word = [], [], False, False
    pending = []  # heredocs waiting for their bodies: (delimiter, strip tabs, index in DOCS)

    def end_word():
        nonlocal word, quoted, in_word
        if in_word:
            raw = "".join(word)
            for piece in (_braces(raw) if not quoted and "{" in raw else [raw]):
                w = Word(piece)
                w.glob = not quoted
                toks.append(w)
        word, quoted, in_word = [], False, False

    i, n = 0, len(text)
    while i < n:
        ch = text[i]
        if ch == "'":
            j = text.find("'", i + 1)
            if j < 0:
                raise Fail("a ' quote isn't closed", 2)
            word.append(text[i + 1:j])
            quoted = in_word = True
            i = j + 1
            continue
        if ch == '"':
            i += 1
            quoted = in_word = True
            while True:
                if i >= n:
                    raise Fail("a \" quote isn't closed", 2)
                c = text[i]
                if c == '"':
                    i += 1
                    break
                if c == "\\" and i + 1 < n and text[i + 1] in '"\\$`\n':
                    if text[i + 1] != "\n":
                        word.append(text[i + 1])
                    i += 2
                    continue
                if c == "`" or (c == "$" and i + 1 < n and (text[i + 1] in "({" or text[i + 1].isalpha()
                                                             or text[i + 1] == "_")):
                    raise Fail(NO_VARS, 2)
                if text.startswith("$?", i):
                    word.append(STATUS)
                    i += 2
                    continue
                word.append(c)
                i += 1
            continue
        if ch == "\\":
            if i + 1 < n:
                if text[i + 1] == "\n":
                    i += 2
                    continue
                word.append(text[i + 1])
                quoted = in_word = True
                i += 2
                continue
            i += 1
            continue
        if ch in " \t\r":
            end_word()
            i += 1
            continue
        if ch == "#" and not in_word:
            while i < n and text[i] != "\n":
                i += 1
            continue
        if ch == "\n":
            end_word()
            i += 1
            for delim, strip, k in pending:
                body = []
                while i < n:
                    j = text.find("\n", i)
                    j = n if j < 0 else j
                    line = text[i:j]
                    i = j + 1
                    if (line.lstrip("\t") if strip else line).rstrip() == delim:
                        break
                    body.append(line.lstrip("\t") if strip else line)
                DOCS[k] = joined(body)
            pending = []
            if toks and not (isinstance(toks[-1], Op) and toks[-1] in (";", "&&", "||", "|")):
                toks.append(Op(";"))
            continue
        if ch == "`" or (ch == "$" and i + 1 < n and (text[i + 1] in "({" or text[i + 1].isalpha()
                                                      or text[i + 1] == "_")):
            raise Fail(NO_VARS, 2)
        if text.startswith("$?", i):
            word.append(STATUS)
            in_word = True
            i += 2
            continue
        if not in_word and ch in "12&>":
            for rx, repl in _REDIRS:
                m = rx.match(text, i)
                if m:
                    toks.extend(Op(r) for r in repl)
                    i = m.end()
                    break
            else:
                m = None
            if m:
                continue
        if ch in ";&|<>":
            end_word()
            if text.startswith("<<", i) and not text.startswith("<<<", i):
                m = _HEREDOC.match(text, i)
                if not m:
                    raise Fail("<< needs a word to end the heredoc, like  cat > f.txt <<'EOF'", 2)
                delim = next(g for g in m.groups()[1:] if g is not None)
                DOCS.append("")
                pending.append((delim, m.group(1) == "-", len(DOCS) - 1))
                toks.append(Op("<<"))
                toks.append(Word(str(len(DOCS) - 1)))
                i = m.end()
                continue
            for op in ("&&", "||", ">>", "<<<", ";", "&", "|", "<", ">"):
                if text.startswith(op, i):
                    break
            toks.append(Op(op))
            i += len(op)
            continue
        if ch in "()" and not in_word:
            raise Fail("( ) grouping, subshells and <(…) aren't supported here: chain commands with && (cd lasts "
                       "only for this command line anyway), or save output to a file first", 2)
        word.append(ch)
        in_word = True
        i += 1
    end_word()
    for delim, strip, k in pending:
        raise Fail(f"the heredoc has no end: put {delim} alone on its own line after the text", 2)
    while toks and isinstance(toks[-1], Op) and toks[-1] == ";":
        toks.pop()
    return toks


def split_list(toks):
    """[(op_before, pipeline_tokens)] where op_before is None, ';', '&&' or '||'."""
    items, cur, op = [], [], None
    for t in toks:
        if isinstance(t, Op) and t in (";", "&&", "||", "&"):
            if t == "&":
                raise Fail("background jobs (&) aren't supported here", 2)
            if not cur:
                if t == ";" and op in (None, ";"):
                    continue
                raise Fail(f"syntax error near {t}", 2)
            items.append((op, cur))
            op, cur = t, []
        else:
            cur.append(t)
    if cur:
        items.append((op, cur))
    elif op in ("&&", "||"):
        raise Fail(f"nothing after {op}", 2)
    return items


def expand(word):
    if STATUS in word:
        w = Word(word.replace(STATUS, str(OPTS["status"])))
        w.glob = word.glob
        word = w
    if getattr(word, "glob", False) and any(c in word for c in "*?[") and not word.startswith("-"):
        hits = sorted(_glob.glob(word, root_dir=os.getcwd(), recursive="**" in word))
        if hits:
            return hits
    return [word]


def run_simple(toks, stdin):
    """One command with its redirects. Returns (out, err, code)."""
    words, out_to, append, in_from = [], None, False, None
    err_out = err_null = out_err = False
    err_to, err_append = None, False
    i = 0
    while i < len(toks):
        t = toks[i]
        if isinstance(t, Op):
            if t in (">", ">>", "<", "<<", "<<<", "__ERR_FILE__", "__ERR_APPEND__"):
                if i + 1 >= len(toks) or isinstance(toks[i + 1], Op):
                    raise Fail(f"nothing after {t.replace('__ERR_FILE__', '2>').replace('__ERR_APPEND__', '2>>')}", 2)
                target = toks[i + 1]
                if t == "<":
                    stdin = read_text(target)
                elif t == "<<":
                    stdin = DOCS[int(target)]
                elif t == "<<<":
                    stdin = target + "\n"
                elif t in (">", ">>"):
                    out_to, append = target, t == ">>"
                else:
                    err_to, err_append = target, t == "__ERR_APPEND__"
                i += 2
                continue
            if t == "__ERR_TO_OUT__":
                err_out = True
            elif t == "__ERR_NULL__":
                err_null = True
            elif t == "__OUT_TO_ERR__":
                out_err = True
            elif t == "|":
                raise Fail("syntax error near |", 2)
            i += 1
            continue
        words.extend(expand(t))
        i += 1
    if not words:
        if out_to is not None:
            write_text(out_to, "", append)
        return "", "", 0
    out, err, code = invoke(words, stdin)
    if err_null:
        err = ""
    if err_to is not None:
        write_text(err_to, err, err_append)
        err = ""
    if err_out:
        out, err = out + err, ""
    if out_err:
        out, err = "", err + out
    if out_to is not None:
        write_text(out_to, out, append)
        out = ""
    return out, err, code


def run_pipeline(toks, stdin):
    parts, cur = [], []
    for t in toks:
        if isinstance(t, Op) and t == "|":
            if not cur:
                raise Fail("syntax error near |", 2)
            parts.append(cur)
            cur = []
        else:
            cur.append(t)
    if not cur:
        raise Fail("nothing after |", 2)
    parts.append(cur)
    data, errs, code, failed = stdin, [], 0, 0
    for p in parts:
        data, err, code = run_simple(p, data)
        errs.append(err)
        failed = code or failed
    return data, "".join(errs), (failed if OPTS["pipefail"] else code)


def run_line(line, stdin=""):
    out, err, code = [], [], 0
    try:
        items = split_list(tokens(line))
    except Fail as f:
        return "", f"{f}\n", f.code
    try:
        for k, (op, toks) in enumerate(items):
            if (op == "&&" and code != 0) or (op == "||" and code == 0):
                continue
            try:
                o, e, code = run_pipeline(toks, stdin)
            except Fail as f:
                o, e, code = "", f"{f}\n", f.code
            out.append(o)
            err.append(e)
            OPTS["status"] = code
            nxt = items[k + 1][0] if k + 1 < len(items) else None
            if code and OPTS["errexit"] and nxt not in ("&&", "||"):
                break
    except _Exit as x:
        code = x.code
    return "".join(out), "".join(err), code


def cut(text):
    if len(text) <= MAX_OUT:
        return text
    head, tail = text[: MAX_OUT // 3], text[-(MAX_OUT * 2 // 3):]
    return f"{head}\n[… {len(text) - len(head) - len(tail):,} characters cut …]\n{tail}"


@chummy.tool
def run_command(command: str) -> str:
    """Run commands in the workspace with Chummy's own small sandboxed shell (not bash): pipes, && || ;, several lines, redirects (> >> < 2>&1 2>/dev/null), heredocs (cat > f <<'EOF'), quotes, globs. Commands: ls cat head tail wc grep find sed awk sort uniq cut tr paste column nl tac rev diff echo printf xargs tee seq tree du mkdir cp mv rm touch cd pwd basename dirname realpath test [ md5sum sha256sum base64 date sleep, and python (scripts, -c, -m, stdin); run  help  or  COMMAND --help  for options. No variables, loops or $(…) (use xargs, awk or python), no network, no installing, nothing outside the workspace.

    command: the command line, like  grep -rn TODO . | head  or  python tests.py
    """
    if not command.strip():
        raise chummy.ToolError("Give a command to run.")
    os.chdir(chummy.WORKSPACE)
    OPTS.update(errexit=False, pipefail=False, status=0)
    DOCS.clear()
    out, err, code = run_line(command)
    text = out + (("\n" if out and not out.endswith("\n") else "") + err if err else "")
    text = cut(text.rstrip("\n"))
    status = f"[exit code {code}]"
    return f"{text}\n{status}" if text else f"(no output)\n{status}"
