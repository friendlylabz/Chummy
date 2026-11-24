# Sandboxes: where tools run

Chummy's loops, model calls and permission checks always run in Chummy
itself. The one piece of user-defined code a loop can start is a **tool call**,
and the sandbox decides where that runs. There's one sandbox for the whole
app, chosen in **Settings → Security and Privacy → Sandbox**. The default is
**Native**.

| Sandbox | What runs a tool call | Real programs | Needs |
|---|---|---|---|
| **Native** (default) | CPython 3.14 for WebAssembly under Wasmtime, built into Chummy | no | nothing (the WebAssembly Python comes with Chummy) |
| **Container** | a fresh Docker or Podman container per call | yes, whatever the image has | Docker or Podman already installed and running |
| **OpenShell** | an NVIDIA OpenShell sandbox, kept per workspace | yes, whatever its image has | the `openshell` program and a working OpenShell gateway |

Chummy never installs Docker, Podman or OpenShell, and never suggests one on
its own. Switching to Container or OpenShell asks you natively; switching back
to Native never asks.

## What's the same in every sandbox

- The tool sees **only the agent's workspace** and its own files, read-only.
  Nothing else on the computer.
- The tool has **no network of its own**. `chummy.http` sends requests to
  Chummy over the tool's stdin/stdout, and Chummy decides each one with the
  same checks everywhere: the Internet switch in Security and Privacy, the
  tool's and the agent's switches, the block's switch, "ask before tools go
  online", and no addresses on this computer or the local network.
- The tool gets **no host environment variables**, only what Chummy passes
  (`TZ`).
- Tools keep **one format** (a PEP 723 Python file, see `tool-format.md`) and
  are always installed and described with the WebAssembly Python, whichever
  sandbox later runs them.

## Which tools a sandbox can run

A tool says what it needs in its header:

- `needs = ["programs"]`: it starts other programs, so it only works in
  Container or OpenShell (the built-in `run_shell` is one).
- `requires-python`: the sandbox's Python must satisfy it (the built-ins need
  3.14 or newer, so a container image needs Python 3.14+).

Settings → Tools and each agent's tool list **grey out** tools the current
sandbox can't run, with the reason. They're never offered to the model (a
block can't turn them on either). A greyed tool can still be switched off,
but not on. If the sandbox isn't ready (Docker isn't running, no image yet,
OpenShell's gateway is unreachable), no tools are offered until a check
passes. Container and OpenShell are checked at start-up, whenever their
settings change, and with **Check again**.

## Native

The original sandbox: see `tool-format.md` → The sandbox. Each call is a
fresh WebAssembly instance with `/workspace`, `/tool`, `/lib` and `/chummy`
preopened; WASI has no sockets and no way to start processes.

## Container

Each call is `docker run --rm -i` (or `podman run`) with:

- `--network none`, `--read-only` root filesystem and a 256 MB `/tmp`;
- `--cap-drop ALL`, `--security-opt no-new-privileges`, `--ipc none`,
  `--pids-limit 256`, memory and CPU limits (Settings), `--log-driver none`
  (the engine keeps no copy of the tool's output);
- exactly three mounts: the workspace at `/workspace` (read/write), the tool
  at `/tool` and Chummy's runner at `/chummy` (both read-only). The Docker
  socket is never mounted;
- with Docker on Linux and macOS, the workspace owner's user and group
  instead of root, so files the tool writes belong to you. Rootless Podman
  maps the container's root to you already;
- `--pull never` and the **approved image ID**. You choose an image name
  (default `python:3.14-slim`) and either **Download image** (`docker pull`)
  or **Use the copy on this computer**; both ask natively and record the
  image's ID. Calls run that ID, so if the name later points to a different
  image nothing changes until you approve again. Changing the name or the
  engine clears the approval.

A call that runs past its time limit is stopped with `docker kill`, and the
container is removed.

## OpenShell

Chummy drives the `openshell` program (the same commands you'd type: `sandbox
create`, `upload`, `exec`, `download`, `delete`), so it works with whatever
gateway your OpenShell is set up for.

- **Policy.** Chummy's sandboxes are created with its own policy: Landlock
  required (`hard_requirement`), the unprivileged `sandbox` user, only
  `/usr`, `/lib` and `/etc` readable, only `/sandbox` and `/tmp` writable, and
  **no network policies at all**, so nothing inside can connect anywhere.
- **One sandbox per workspace**, named from a hash of the workspace's
  location (two agents never share one), kept between calls because creating
  one takes a while. **Remove them** in Settings deletes them all; Chummy makes
  new ones when needed.
- **The workspace is copied in and out.** OpenShell can't mount a folder, so
  for each call Chummy copies the workspace (through the agent's jail, links
  left out) into `/sandbox/<call id>/workspace`, runs the tool there, copies
  the result back and removes the call folder. Bringing changes back:
  - new and changed files are written and files the tool deleted are deleted,
    **only where the workspace still matches what was copied in**; if you (or
    another chat) changed a file during the call, your version is kept and
    the tool's log says so;
  - links and special files that come back are ignored;
  - changes come back only from a call that finished; a call that crashed or
    timed out changes nothing;
  - workspaces over 20,000 files or 512 MB can't be copied (the tool reports
    it), so OpenShell suits small project folders best.
- Because one sandbox serves one workspace for many calls, a tool could leave
  something in `/tmp` that a later call in the same workspace sees. It never
  reaches another agent's sandbox.

The OpenShell backend is tested against a stand-in for the `openshell`
program (`crates/tools/tests/fake_openshell.py`), built from the real CLI's
argument definitions. It hasn't been run against a real OpenShell gateway yet.

## The programs Chummy starts

For Container and OpenShell, Chummy itself starts `docker`/`podman`/`openshell`
outside any sandbox, to create the sandbox. Chummy finds them on `PATH` and in
the usual install places. You can point it at one with **Choose…**, which only
works through a native file picker (a window can't type a path in), refuses
files inside any agent's workspace or the Chummy folder (a tool could replace
those), and asks you to confirm. Chummy passes these programs only the
environment variables they need (`PATH`, `HOME`, `DOCKER_HOST`, proxies and
the like).

## Adding a sandbox

`crates/tools/src/sandbox/`: implement `SandboxBackend` (`kind`, `check`,
`call`), add a `SandboxKind` variant and its options to `SandboxSettings`, and
build it in `AppState::build_sandbox`. Process-based backends can reuse
`process::run`, which speaks the tool line protocol over a child's
stdin/stdout, and `sync` if they need to copy the workspace.
