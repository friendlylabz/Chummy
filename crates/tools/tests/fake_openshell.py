"""A stand-in for NVIDIA OpenShell's `openshell` program, for tests.

It supports the commands Chummy uses, with the same arguments as the real
CLI (crates/openshell-cli in NVIDIA/OpenShell): status; sandbox get, create,
list, delete, upload, download and exec. Each sandbox is a folder; its
/sandbox is <state>/<name>/sandbox, and commands run with the host's Python
after "/sandbox" in paths and environment values is rewritten to that
folder. It isn't a sandbox at all: it only checks Chummy's side.

The state folder is named in a file called `state-dir` next to this script
(Chummy starts `openshell` with a cleaned environment). A file called
`gateway-down` there makes `status` fail.
"""

import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(sys.argv[0]))
with open(os.path.join(HERE, "state-dir")) as f:
    STATE = f.read().strip()
os.makedirs(STATE, exist_ok=True)


def log(args):
    with open(os.path.join(STATE, "calls.log"), "a") as f:
        f.write(" ".join(args) + "\n")


def fail(msg, code=1):
    print(msg, file=sys.stderr)
    sys.exit(code)


def root(name):
    return os.path.join(STATE, name, "sandbox")


def tr(name, s):
    if s == "/sandbox" or s.startswith("/sandbox/"):
        return root(name) + s[len("/sandbox"):]
    return s


def into(src, dest):
    """cp-style: an existing folder (or one ending in /) receives src's name."""
    if dest.endswith("/") or os.path.isdir(dest):
        dest = os.path.join(dest, os.path.basename(src.rstrip("/")))
    if os.path.isdir(src):
        shutil.copytree(src, dest, symlinks=True)
    else:
        shutil.copy2(src, dest, follow_symlinks=False)


def main(argv):
    log(argv)
    if argv[:1] == ["status"]:
        if os.path.exists(os.path.join(HERE, "gateway-down")):
            fail("Error: failed to connect to gateway: connection refused")
        print("Status: Connected")
        return 0
    if argv[:1] != ["sandbox"] or len(argv) < 2:
        fail("unknown command")
    cmd, rest = argv[1], argv[2:]
    if cmd == "get":
        if not os.path.isdir(root(rest[0])):
            fail(f"sandbox {rest[0]} not found")
        print(f"Name: {rest[0]}\nPhase: Ready")
        return 0
    if cmd == "create":
        name = rest[rest.index("--name") + 1]
        policy = rest[rest.index("--policy") + 1]
        shutil.copy(policy, os.path.join(STATE, "last-policy.yaml"))
        os.makedirs(root(name), exist_ok=True)
        print(f"Created sandbox {name}")
        return 0
    if cmd == "list":
        for n in sorted(os.listdir(STATE)):
            if os.path.isdir(root(n)):
                print(n)
        return 0
    if cmd == "delete":
        for n in rest:
            shutil.rmtree(os.path.join(STATE, n))
        return 0
    if cmd == "upload":
        name, local, dest = rest[0], rest[1], rest[2]
        into(local, tr(name, dest))
        return 0
    if cmd == "download":
        name, src, dest = rest[0], rest[1], rest[2]
        into(tr(name, src), dest)
        return 0
    if cmd == "exec":
        sep = rest.index("--")
        opts, command = rest[:sep], rest[sep + 1:]
        name = opts[opts.index("-n") + 1]
        workdir = None
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        i = 0
        while i < len(opts):
            if opts[i] == "--workdir":
                workdir = tr(name, opts[i + 1])
                i += 1
            elif opts[i] == "--env":
                k, _, v = opts[i + 1].partition("=")
                env[k] = tr(name, v)
                i += 1
            i += 1
        command = [tr(name, c) for c in command]
        if command[:2] == ["rm", "-rf"]:
            shutil.rmtree(command[2], ignore_errors=True)
            return 0
        if command and command[0] == "python3":
            command[0] = sys.executable
        return subprocess.run(command, cwd=workdir or root(name), env=env).returncode
    fail(f"unknown sandbox command {cmd}")


sys.exit(main(sys.argv[1:]))
