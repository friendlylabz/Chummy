# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "run_shell"
# title = "Run shell command"
# version = "1.0.0"
# timeout = 120
# risky = true
# needs = ["programs"]
# ///
"""Runs a real shell command (`sh -c`) in the agent's workspace.

Unlike run_command (a small shell written in Python), this starts real
programs, so it needs a sandbox that has them: Container or OpenShell. In
the Native (WebAssembly) sandbox it's shown greyed out and never offered.

It still runs inside that sandbox: only the workspace is there, there's no
network, and whatever the image has installed is what's available.

Potentially unsafe: the model can delete or overwrite anything in the
workspace in one command, and run code it wrote (until the time limit).
"""

import os

import chummy

MAX_OUT = 30_000
TIME_LIMIT = 110


def cut(text):
    if len(text) <= MAX_OUT:
        return text
    head, tail = text[: MAX_OUT // 3], text[-(MAX_OUT * 2 // 3):]
    return f"{head}\n[… {len(text) - len(head) - len(tail):,} characters cut …]\n{tail}"


@chummy.tool
def run_shell(command: str) -> str:
    """Run a real shell command in the workspace with sh -c: any program installed in the sandbox (git, make, compilers, test runners…). No network and nothing outside the workspace.

    command: the command line, like  make test  or  ls -la src | head
    """
    # Imported here: the WebAssembly Python that reads tool descriptions
    # can't start programs.
    import subprocess

    if not command.strip():
        raise chummy.ToolError("Give a command to run.")
    os.chdir(chummy.WORKSPACE)
    env = {
        "PATH": os.environ.get("PATH") or "/usr/local/bin:/usr/bin:/bin",
        "HOME": "/tmp",
        "LANG": "C.UTF-8",
    }
    if os.environ.get("TZ"):
        env["TZ"] = os.environ["TZ"]
    try:
        p = subprocess.run(
            ["sh", "-c", command],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env,
            timeout=TIME_LIMIT,
        )
    except FileNotFoundError:
        raise chummy.ToolError("This sandbox has no sh, so it can't run shell commands.")
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"").decode("utf-8", "replace")
        text = cut(out.rstrip("\n"))
        return f"{text}\n[stopped after {TIME_LIMIT} seconds]" if text else f"[stopped after {TIME_LIMIT} seconds]"
    text = cut(p.stdout.decode("utf-8", "replace").rstrip("\n"))
    status = f"[exit code {p.returncode}]"
    return f"{text}\n{status}" if text else f"(no output)\n{status}"
