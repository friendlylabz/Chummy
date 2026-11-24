# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "run_command"
# title = "Run command"
# version = "1.0.0"
# timeout = 120
# risky = true
# ///
"""Runs a shell-style command line inside the sandbox.

There's no real shell in the WebAssembly sandbox (and Chummy never runs
anything outside it), so this is a small shell of its own: pipes (|),
&&, ||, ;, redirects (<, >, >>, 2>&1, 2>/dev/null), quotes, globs, cd, and
a set of common commands written in Python, plus `python` itself, which runs
scripts in the workspace in this same sandboxed interpreter. Everything
stays inside the agent's workspace, with no network.

Potentially unsafe: the model can delete or overwrite anything in the
workspace in one command, and run code it wrote (until the time limit).
"""

import difflib
import fnmatch
import glob as _glob
import io
import os
import re
import runpy
import shlex
import shutil
import sys
import time
import traceback

import chummy

MAX_OUT = 30_000


class Fail(Exception):
    """A command's error: message for stderr, and the exit code."""

    def __init__(self, msg, code=1):
        super().__init__(msg)
        self.code = code


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
    if os.path.isdir(full):
        raise Fail(f"{path}: is a folder")
    try:
        with open(full, encoding="utf-8", errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        raise Fail(f"{path}: no such file")


def inputs(files, stdin):
    if not files or files == ["-"]:
        return [(None, stdin)]
    return [(f, read_text(f)) for f in files]


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
                key = a[:2]
                if len(a) > 2:
                    opts[key] = a[2:]
                else:
                    i += 1
                    if i >= len(args):
                        raise Fail(f"option {a} needs a value", 2)
                    opts[key] = args[i]
            elif a.startswith("--"):
                opts[a] = True
            else:
                for ch in a[1:]:
                    if ch not in known:
                        raise Fail(f"unknown option -{ch}", 2)
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
        if re.fullmatch(r"-n[+]?\d+", a):
            from_start = "+" in a
            n = int(a[2:].lstrip("+"))
        elif re.fullmatch(r"-\d+", a):
            n = int(a[1:])
        else:
            rest.append(a)
        i += 1
    return n, from_start, rest


# ---------------------------------------------------------------- commands

def c_echo(args, stdin):
    nl = True
    if args[:1] == ["-n"]:
        nl, args = False, args[1:]
    return " ".join(args) + ("\n" if nl else "")


def c_cat(args, stdin):
    o, files = flags(args, "n")
    out = "".join(t for _, t in inputs(files, stdin))
    if o.get("-n"):
        out = "".join(f"{i:>6}\t{l}\n" for i, l in enumerate(out.splitlines(), 1))
    return out


def c_head(args, stdin):
    n, _, files = count_opt(args)
    return "".join("".join(t.splitlines(True)[:n]) for _, t in inputs(files, stdin))


def c_tail(args, stdin):
    n, from_start, files = count_opt(args)
    out = []
    for _, t in inputs(files, stdin):
        lines = t.splitlines(True)
        out.append("".join(lines[n - 1:] if from_start else lines[-n:] if n else []))
    return "".join(out)


def c_wc(args, stdin):
    o, files = flags(args, "lwc")
    if not o:
        o = {"-l": True, "-w": True, "-c": True}
    rows = []
    for name, t in inputs(files, stdin):
        parts = []
        if o.get("-l"):
            parts.append(str(t.count("\n")))
        if o.get("-w"):
            parts.append(str(len(t.split())))
        if o.get("-c"):
            parts.append(str(len(t.encode())))
        rows.append(" ".join(parts) + (f" {name}" if name else ""))
    return "\n".join(rows) + "\n"


def c_grep(args, stdin):
    o, rest = flags(args, "inrvclEFwHhoRs", with_value=("-e",))
    if "-e" in o:
        pattern, files = o["-e"], rest
    elif rest:
        pattern, files = rest[0], rest[1:]
    else:
        raise Fail("usage: grep [options] pattern [files]", 2)
    pat = re.escape(pattern) if o.get("-F") else pattern.replace(r"\|", "|").replace(r"\(", "(").replace(r"\)", ")")
    if o.get("-w"):
        pat = rf"\b(?:{pat})\b"
    try:
        rx = re.compile(pat, re.IGNORECASE if o.get("-i") else 0)
    except re.error as e:
        raise Fail(f"bad pattern: {e}", 2)
    recursive = o.get("-r") or o.get("-R")
    targets = []
    if not files:
        targets = [(None, stdin)] if not recursive else None
        files = ["."] if recursive else []
    if targets is None or files:
        targets = []
        for f in files:
            full = resolve(f)
            if os.path.isdir(full):
                if not recursive:
                    raise Fail(f"{f}: is a folder (use -r)")
                for folder, dirs, names in os.walk(full):
                    dirs[:] = sorted(d for d in dirs if not d.startswith(".") and d not in ("node_modules", "__pycache__"))
                    for n in sorted(names):
                        p = os.path.join(folder, n)
                        try:
                            with open(p, encoding="utf-8") as fh:
                                targets.append((shown(p), fh.read()))
                        except (UnicodeDecodeError, OSError):
                            pass
            else:
                targets.append((f, read_text(f)))
    show_name = (len(targets) > 1 or recursive) and not o.get("-h") or o.get("-H")
    out, found = [], False
    for name, text in targets:
        n_hits = 0
        for i, line in enumerate(text.splitlines(), 1):
            hit = bool(rx.search(line)) != bool(o.get("-v"))
            if not hit:
                continue
            found, n_hits = True, n_hits + 1
            if o.get("-l") or o.get("-c"):
                continue
            body = "\n".join(m.group(0) for m in rx.finditer(line)) if o.get("-o") else line
            prefix = (f"{name}:" if show_name and name else "") + (f"{i}:" if o.get("-n") else "")
            out.append(prefix + body)
        if o.get("-l") and n_hits:
            out.append(name or "(standard input)")
        if o.get("-c"):
            out.append((f"{name}:" if show_name and name else "") + str(n_hits))
    if not found and not o.get("-c"):
        raise Fail("", 1)
    return "\n".join(out) + ("\n" if out else "")


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


def c_find(args, stdin):
    roots, i = [], 0
    while i < len(args) and not args[i].startswith("-"):
        roots.append(args[i])
        i += 1
    roots = roots or ["."]
    name = iname = kind = None
    depth = None
    while i < len(args):
        a = args[i]
        v = args[i + 1] if i + 1 < len(args) else None
        if a == "-name":
            name = v
        elif a == "-iname":
            iname = v
        elif a == "-type":
            kind = v
        elif a == "-maxdepth":
            depth = int(v)
        else:
            raise Fail(f"{a} isn't supported here (use -name, -iname, -type, -maxdepth)", 2)
        i += 2
    out = []
    for r in roots:
        top = resolve(r)
        for folder, dirs, files in os.walk(top):
            level = os.path.relpath(folder, top).count(os.sep) + (0 if folder == top else 1)
            dirs[:] = sorted(d for d in dirs if d not in (".git", "node_modules", "__pycache__"))
            if depth is not None and level >= depth:
                dirs[:] = []
            items = [(d, "d") for d in dirs] + [(f, "f") for f in sorted(files)]
            if folder == top:
                items = [(".", "d")] + items if depth != 0 else [(".", "d")]
            for n, k in items:
                p = folder if n == "." else os.path.join(folder, n)
                rel = os.path.relpath(p, top)
                label = r if rel == "." else r.rstrip("/") + "/" + rel
                if kind and kind != k:
                    continue
                base = os.path.basename(label)
                if name and not fnmatch.fnmatchcase(base, name):
                    continue
                if iname and not fnmatch.fnmatch(base.lower(), iname.lower()):
                    continue
                out.append(label)
    return "\n".join(dict.fromkeys(out)) + ("\n" if out else "")


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


def c_sort(args, stdin):
    o, files = flags(args, "rnuf")
    lines = [l for _, t in inputs(files, stdin) for l in t.splitlines()]

    def key(l):
        if o.get("-n"):
            m = re.match(r"\s*(-?\d+(\.\d+)?)", l)
            return (0, float(m.group(1))) if m else (1, l)
        return l.lower() if o.get("-f") else l

    lines.sort(key=key, reverse=bool(o.get("-r")))
    if o.get("-u"):
        lines = list(dict.fromkeys(lines))
    return "\n".join(lines) + ("\n" if lines else "")


def c_uniq(args, stdin):
    o, files = flags(args, "c")
    out, prev, n = [], None, 0
    for _, t in inputs(files, stdin):
        for l in t.splitlines() + [None]:
            if l == prev:
                n += 1
                continue
            if prev is not None:
                out.append(f"{n:>7} {prev}" if o.get("-c") else prev)
            prev, n = l, 1
    return "\n".join(out) + ("\n" if out else "")


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


def c_tr(args, stdin):
    o, sets = flags(args, "d")
    if o.get("-d"):
        return "".join(ch for ch in stdin if ch not in sets[0])
    if len(sets) != 2:
        raise Fail("usage: tr set1 set2  or  tr -d set", 2)
    def expand(s):
        return re.sub(r"(.)-(.)", lambda m: "".join(chr(c) for c in range(ord(m.group(1)), ord(m.group(2)) + 1)), s)
    a, b = expand(sets[0]), expand(sets[1])
    b = (b + b[-1:] * len(a))[: len(a)]
    return stdin.translate(str.maketrans(a, b))


def c_diff(args, stdin):
    o, files = flags(args, "u")
    if len(files) != 2:
        raise Fail("usage: diff file1 file2", 2)
    a, b = read_text(files[0]), read_text(files[1])
    d = list(difflib.unified_diff(a.splitlines(True), b.splitlines(True), files[0], files[1]))
    if not d:
        return ""
    return _Diff("".join(d))


class _Diff(str):
    """diff's output, with exit code 1 (files differ)."""


def c_pwd(args, stdin):
    rel = os.path.relpath(os.getcwd(), chummy.WORKSPACE)
    return chummy.WORKSPACE + ("" if rel == "." else "/" + rel) + "\n"


def c_cd(args, stdin):
    target = resolve(args[0] if args else ".")
    if not os.path.isdir(target):
        raise Fail(f"{args[0]}: no such folder")
    os.chdir(target)
    return ""


def c_date(args, stdin):
    return time.strftime("%a %b %d %H:%M:%S %Z %Y") + "\n"


def c_sleep(args, stdin):
    time.sleep(min(float(args[0]) if args else 0, 60))
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


class _Result:
    def __init__(self, out, err, code):
        self.out, self.err, self.code = out, err, code


def c_pip(args, stdin):
    raise Fail("packages can't be installed in this sandbox; only Python's standard library is available", 127)


def c_shell(args, stdin):
    """bash -c "…" / sh -c "…": run the inner command line here."""
    if len(args) >= 2 and args[0] == "-c":
        return _Result(*run_line(args[1], stdin))
    raise Fail("only bash -c \"command\" works here (there's no real shell)", 2)


COMMANDS = {
    "echo": c_echo, "cat": c_cat, "head": c_head, "tail": c_tail, "wc": c_wc, "grep": c_grep,
    "ls": c_ls, "find": c_find, "mkdir": c_mkdir, "rm": c_rm, "cp": c_cp, "mv": c_mv, "touch": c_touch,
    "sort": c_sort, "uniq": c_uniq, "cut": c_cut, "tr": c_tr, "diff": c_diff, "pwd": c_pwd, "cd": c_cd,
    "date": c_date, "sleep": c_sleep, "true": c_true, "false": c_false,
    "python": c_python, "python3": c_python, "pip": c_pip, "pip3": c_pip, "bash": c_shell, "sh": c_shell,
}


# ------------------------------------------------------------------ parser

OPS = {";", "&&", "||", "|", "<", ">", ">>"}


class Word(str):
    """A word of the command line; `glob` is false if it was quoted."""

    glob = True


def tokens(line):
    # Redirections of stderr are rewritten to words the parser knows.
    line = re.sub(r"(?<![\w-])2>&1", " __ERR_TO_OUT__ ", line)
    line = re.sub(r"(?<![\w-])2>\s*/dev/null", " __ERR_NULL__ ", line)
    # Non-POSIX mode keeps the quotes, so quoted words aren't globbed.
    lex = shlex.shlex(line, posix=False, punctuation_chars=";&|<>")
    lex.whitespace_split = True
    lex.commenters = "#"
    out = []
    try:
        for raw in lex:
            if raw in OPS or raw in ("&", "&&", "||"):
                out.append(raw)
                continue
            parts = shlex.split(raw, comments=False, posix=True)
            w = Word(parts[0] if parts else "")
            w.glob = not any(q in raw for q in "'\"\\")
            out.append(w)
    except ValueError as e:
        raise Fail(f"can't parse the command: {e}", 2)
    return out


def split_list(toks):
    """[(op_before, pipeline_tokens)] where op_before is None, ';', '&&' or '||'."""
    items, cur, op = [], [], None
    for t in toks:
        if t in (";", "&&", "||", "&"):
            if t == "&":
                raise Fail("background jobs (&) aren't supported here", 2)
            if cur:
                items.append((op, cur))
            op, cur = t, []
        else:
            cur.append(t)
    if cur:
        items.append((op, cur))
    return items


def expand(word):
    if getattr(word, "glob", False) and any(c in word for c in "*?[") and not word.startswith("-"):
        base = os.getcwd()
        hits = sorted(_glob.glob(word, root_dir=base))
        if hits:
            return hits
    return [word]


def run_simple(toks, stdin):
    """One command with its redirects. Returns (out, err, code)."""
    words, out_to, append, in_from, err_out, err_null = [], None, False, None, False, False
    i = 0
    while i < len(toks):
        t = toks[i]
        if t in (">", ">>", "<"):
            if i + 1 >= len(toks):
                raise Fail(f"nothing after {t}", 2)
            if t == "<":
                in_from = toks[i + 1]
            else:
                out_to, append = toks[i + 1], t == ">>"
            i += 2
            continue
        if t == "__ERR_TO_OUT__":
            err_out = True
        elif t == "__ERR_NULL__":
            err_null = True
        else:
            words.extend(expand(t))
        i += 1
    if in_from is not None:
        stdin = read_text(in_from)
    if not words:
        return "", "", 0
    name, args = words[0], words[1:]
    fn = COMMANDS.get(name)
    if fn is None:
        msg = (
            f"{name}: command not found. This is a small built-in shell inside Chummy's sandbox, not a real one. "
            f"Available: {', '.join(sorted(COMMANDS))}. Use python for anything else."
        )
        return "", msg + "\n", 127
    out, err, code = "", "", 0
    try:
        r = fn(args, stdin)
        if isinstance(r, _Result):
            out, err, code = r.out, r.err, r.code
        elif isinstance(r, _Diff):
            out, code = str(r), 1
        else:
            out = r
    except Fail as e:
        err, code = (f"{name}: {e}\n" if str(e) else ""), e.code
    except (OSError, ValueError) as e:
        err, code = f"{name}: {e}\n", 1
    if err_null:
        err = ""
    if err_out:
        out, err = out + err, ""
    if out_to is not None:
        if out_to != "/dev/null":
            full = resolve(out_to)
            with open(full, "a" if append else "w", encoding="utf-8") as f:
                f.write(out)
        out = ""
    return out, err, code


def run_pipeline(toks, stdin):
    parts, cur = [], []
    for t in toks:
        if t == "|":
            parts.append(cur)
            cur = []
        else:
            cur.append(t)
    parts.append(cur)
    data, errs, code = stdin, [], 0
    for p in parts:
        data, err, code = run_simple(p, data)
        errs.append(err)
    return data, "".join(errs), code


def run_line(line, stdin=""):
    out, err, code = [], [], 0
    for op, toks in split_list(tokens(line)):
        if (op == "&&" and code != 0) or (op == "||" and code == 0):
            continue
        try:
            o, e, code = run_pipeline(toks, stdin)
        except Fail as f:
            o, e, code = "", f"{f}\n", f.code
        out.append(o)
        err.append(e)
    return "".join(out), "".join(err), code


def cut(text):
    if len(text) <= MAX_OUT:
        return text
    head, tail = text[: MAX_OUT // 3], text[-(MAX_OUT * 2 // 3):]
    return f"{head}\n[… {len(text) - len(head) - len(tail):,} characters cut …]\n{tail}"


@chummy.tool
def run_command(command: str) -> str:
    """Run a command line in the workspace, like a small sandboxed shell: pipes, &&, ;, redirects, and common commands (ls, cat, grep, find, head, tail, wc, mkdir, cp, mv, rm, sort, diff…) plus python to run scripts. No network, no installing packages, nothing outside the workspace.

    command: the command line, like  python tests.py  or  grep -n TODO *.py | head
    """
    if not command.strip():
        raise chummy.ToolError("Give a command to run.")
    os.chdir(chummy.WORKSPACE)
    out, err, code = run_line(command)
    text = out + (("\n" if out and not out.endswith("\n") else "") + err if err else "")
    text = cut(text.rstrip("\n"))
    status = f"[exit code {code}]"
    return f"{text}\n{status}" if text else f"(no output)\n{status}"
