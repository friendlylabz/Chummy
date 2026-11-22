# chummyd: plan for the background job service

Status: **plan only.** Nothing here is built yet. Pass 5 built the parts of
Chummy that chummyd will reuse. This document is the master plan for
building chummyd, and the reference for keeping Chummy's code ready for it.

## What chummyd is

chummyd is a headless service that runs agents' loops as jobs: on schedules,
when files change, on request through an authenticated API, and in a queue.
It's for people who want jobs to run when the Chummy app is closed, on
another machine (a home server, a workstation, a small VPS), or around the
clock.

The Chummy app stays the place where you **build and run things while it's
open**: loops, agents, tools, chats, and the jobs that run in its tray mode.
chummyd is optional, installed separately, and configured from the app.

**Design goals, in order:**

1. **Secure at every level.** chummyd is a long-running program that runs
   agents unattended and, if you allow it, listens for connections. Every
   connection is authenticated, every action is authorized and recorded, and
   every limit Chummy has still applies.
2. **Same behavior as the app.** The same loop engine, scheduler, runner,
   tool sandbox and file formats, from the same crates. A job behaves the
   same in the app and in chummyd.
3. **One master config.** chummyd's behavior comes from one file, made and
   exported by the Chummy app, and checked strictly at start-up.
4. **Portable and self-contained.** One base folder, chosen when it's
   installed (there's no default location), holds everything: config, agents'
   folders, jobs, history. Move the folder and chummyd moves with it.

## Installing and running

- **A separate install**, built from the same Rust workspace (`crates/chummyd`),
  with no webview and no Tauri. Packages:
  - a single binary for each OS;
  - `.deb` and `.rpm` with a systemd unit;
  - an MSI that installs a Windows service;
  - a macOS pkg with a launchd daemon;
  - a container image.
- **It never runs as root or Administrator.** It refuses to start that way.
  Packages create a dedicated user (`chummyd`). On Windows it uses a virtual
  service account. A per-user mode (systemd `--user`, a per-user launch agent,
  a Windows logon task) is supported for desktops.
- **The base folder is required:** `chummyd run --base <folder>` (or
  `CHUMMYD_BASE`). There's no default location.
  - `chummyd init --base <folder>` creates the layout and makes it private:
    0700 on Unix, and on Windows an access list for the service account and
    the installing user only.
  - chummyd refuses a base folder that other users can write to, or that's
    inside a protected location (the same rules as an agent's folder).
- **One instance per base folder**, enforced with an OS lock on
  `run/chummyd.lock` (as the app does with the Chummy folder).
- **Command line:**
  - `init`: create a base folder.
  - `check`: validate the config and every file it names.
  - `run`: start.
  - `status`: show what's running.
  - `pair`: make a one-time pairing code.
  - `keys list|revoke`: manage paired clients.
  - `export-audit`: write out the audit log.

### Base folder layout

```text
<base>/
  chummyd.json          the master config (see below)
  secrets/              one file per secret, 0600 (when not in a keychain or env)
  loops/                loop files (chummy.loop), pinned by hash in the config
  jobs/                 job files, state.json, history/: the same JobStore as the app
  jails/<agent>/        each agent's folder: always here, never elsewhere
  tools/                the WebAssembly Python and the built-in tools
  models/               optional: weights for the built-in engine
  runs/<run id>.jsonl   each run's events, for viewing and audit (pruned by age)
  asks/                 questions waiting for an answer, kept across restarts
  audit/                append-only, hash-chained log of every authenticated action
  run/                  lock file and the local socket
```

**Agents' folders are always inside `jails/`.** On a desktop, the app lets
you pick any allowed folder. In chummyd the rule is stricter: an agent's
folder is `jails/<name>/`, created by chummyd, and nothing else. This keeps
the whole blast radius inside one folder the operator chose, and makes
backups and moving the folder trivial. Files to work on are copied in, or
written there by other jobs; there are no links out (the jail refuses
them anyway).

## The master config: `chummyd.json`

One JSON file, `format: "chummyd.config"`, `version: 1`. It follows the same
rules as the other formats: strict (unknown fields refused), versioned
(newer versions refused), and size-limited. chummyd reads it at start-up and
refuses to start if anything in it, or in any file it names, fails to
validate. `chummyd check` runs the same validation without starting.

```json
{
  "format": "chummyd.config",
  "version": 1,
  "instance": { "name": "home-server", "time_zone": "America/New_York" },
  "limits": { "max_runs": 2, "max_queued": 200, "daily_model_calls": 2000 },
  "listen": {
    "local": true,
    "remote": null
  },
  "clients": [
    { "name": "Laptop", "key": "ed25519:…", "role": "admin" },
    { "name": "Phone bridge", "key": "ed25519:…", "role": "asker" }
  ],
  "models": [
    { "id": "local-qwen", "kind": "open_ai_compatible", "base_url": "http://127.0.0.1:8080/v1",
      "model": "qwen3-4b", "secret": null },
    { "id": "claude", "kind": "anthropic", "base_url": "https://api.anthropic.com",
      "model": "claude-…", "secret": { "file": "secrets/anthropic" } }
  ],
  "loops": [
    { "id": "digest", "file": "loops/digest.json", "sha256": "…" }
  ],
  "agents": [
    {
      "id": "researcher", "name": "Researcher", "loop": "digest", "model": "local-qwen",
      "jail": "researcher",
      "tools": { "read_file": {}, "fetch_page": { "network": true } },
      "confirm_online_tools": true,
      "shims_may_approve_tools": false
    }
  ],
  "network": { "tools": true },
  "jobs": [
    { "file": "jobs/morning-digest.json", "sha256": "…" }
  ],
  "executor": { "enabled": false },
  "shims": [],
  "audit": { "keep_days": 365 }
}
```

### Rules

- **The config is the root of trust.** Loop and job files are pinned by
  SHA-256. A file that doesn't match its hash stops start-up. This keeps a
  stray edit or a tampered file from changing what runs.
- **Permissions are per agent and explicit**, stricter than in the app: each
  agent lists its tools, each tool's internet access, and whether online
  tools ask first. A global `network.tools` switch must also be on.
  Everything defaults to off.
- **Secrets are references:**
  - a file in `secrets/` (0600, checked at start-up);
  - an environment variable;
  - the OS keychain when chummyd runs per user.

  Secrets are never put in the config itself, never logged, and never
  returned by the API.
- **Changes are explicit.** chummyd doesn't watch its config. It's re-read on
  `SIGHUP` or `chummyd reload`, or applied through the admin API, and the
  whole new config is validated before any of it takes effect. Every change
  is recorded in the audit log with a diff.

### Made by the Chummy app

The app gets an **Export for chummyd** flow (Settings → Background services):

1. Pick the agents, loops, jobs and model connections to include.
2. For each agent, set its jail name and its tools.
   - The app's permissions are the starting point, but each is shown and
     confirmed again.
   - Online tools default to asking.
3. For each model connection with a key, choose how chummyd gets the key:
   - a secret file to fill in on the server;
   - an environment variable name;
   - or, if pushing over a paired connection, send it once, encrypted
     inside the authenticated session.
4. The app writes a **bundle**: `chummyd.json` plus the `loops/` and `jobs/`
   files it names, hashes filled in. Alternatively it pushes the bundle to a
   paired instance through the admin API, after showing a diff of what will
   change.

The config is also plain JSON, so it can be written by hand or kept in
version control. The app can open an existing one, edit it and export it
again.

## File formats

chummyd reads the same files the app does, with the same code:

| Format | Read by | Notes |
|--------|---------|-------|
| `chummy.loop` v1 | `chummy-flow` | Unchanged. Loops narrow permissions, never widen them. |
| `chummy.job` v1 | `chummy-scheduler` | Unchanged. `agent` names an agent in `chummyd.json`. |
| Tool files (PEP 723 + `[tool.chummy]`) | `chummy-tools` | Only the built-in suite, as in the app. No upload. |
| `chummyd.config` v1 | `chummyd` | New, see above. |
| Job state and history | `chummy-scheduler` (`JobStore`) | The same files, so history moves between app and daemon. |

Any change to a shared format is made in its crate, documented in `docs/`,
and versioned. Both programs refuse versions newer than they know.

## Talking to chummyd

All clients use one protocol: the Chummy app, the CLI, shims, and the
executor API.

### Transport

- **Local (on by default):** a Unix domain socket at `run/chummyd.sock`
  (mode 0600, in a 0700 folder), or a Windows named pipe.
  - Socket: the peer's user is checked (`SO_PEERCRED` / `getpeereid`).
  - Named pipe: an access list for the service account and listed users;
    remote clients are rejected (`PIPE_REJECT_REMOTE_CLIENTS`); and it's
    created as the first instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`) so no
    other program can claim the name first.
  - Nothing on the machine is reachable over TCP unless remote access is on.
- **Remote (off by default):** TLS 1.3 only (rustls), never plain TCP or HTTP.
  - chummyd makes its own certificate at `init`; clients pin its fingerprint
    when pairing.
  - It binds to a configured address only, never "all interfaces" by
    default. The recommended setup is still to reach chummyd over SSH, a VPN
    (WireGuard, Tailscale) or a reverse proxy you already trust, and to keep
    `remote` off.
- **No browser can reach it.** There's no HTTP API on localhost, so a web page
  can't talk to chummyd (no DNS rebinding, no CSRF).

### Authentication

- **Pairing.** `chummyd pair --role operator` prints a one-time code (random,
  single use, expires in 10 minutes).
  - The client makes an ed25519 key pair and keeps the private key in its OS
    keychain.
  - It sends the code and its public key; chummyd records the public key
    under the client's name and role.
  - Remote pairing also checks the server's certificate fingerprint, shown
    on both sides.
- **Every connection** proves the client holds its key: chummyd sends a
  random challenge, the client signs it (together with the TLS channel
  binding when remote), and chummyd checks it against the paired keys. No
  reusable bearer tokens exist, so there's nothing to steal from a config
  file.
- **Keys are revocable** (`chummyd keys revoke`, or from the app), and
  revoking one ends that client's sessions at once.

### Authorization

| Role | May |
|------|-----|
| `viewer` | See jobs, runs, history, questions (no answers), the redacted config. |
| `asker` | See and answer questions. For shims; can't see files or config. |
| `operator` | All of the above, plus run, stop, pause and resume jobs, and answer questions. |
| `executor` | Submit loop runs through the executor API (below), as the agents it's allowed. |
| `admin` | All of the above, plus change the config, agents, loops, jobs and keys. |

Roles can be limited to some agents. Every authenticated request is
written to the audit log: who, what, which agent or job, and the result.

### Protocol

JSON-RPC 2.0 over length-prefixed frames on the socket or TLS stream:

- **A `hello` exchange** sets the protocol version and capabilities. Both
  sides refuse versions they don't know.
- **Limits:** frame size (1 MB), message rate, concurrent subscriptions and
  idle time, all with clear errors.
- **Shared types:** the same `serde` types the app already uses (`JobView`,
  `RunRecord`, `Pending`, `Event`, `Job`, `LoopFile`) with the same checks.
- **Calls:**
  - `overview`
  - `jobs.save|delete|enable|run_now|stop|history`
  - `runs.subscribe` (live events) and `runs.journal` (past events)
  - `asks.list|answer|subscribe`
  - `loops.list|get|put`
  - `agents.list`
  - `config.get` (secrets redacted)
  - `config.apply` (admin; validated, diffed, audited)
  - `executor.run`

### The executor API

"Input a loop and its inputs":

- `executor.run { agent, loop: <loop id or inline chummy.loop>, message, overrides }`
  queues a run as that agent and returns a run id to follow with
  `runs.subscribe`.
- **Off by default** (`executor.enabled`), and only for clients with the
  `executor` role, limited to listed agents.
- **An inline loop gets exactly the agent's permissions:** the agent's jail,
  model and tools, and the "online tools ask first" rule. It can narrow them
  and nothing else, just like an imported loop in the app.
- **Executor runs share the queue and limits** with jobs: one run per agent
  at a time, `max_runs`, and the daily model-call budget.

## Questions for the user: waiting and answering

Questions come from *Ask me* blocks and tool approvals, as in the app. They
go through the same `AskBroker` from `chummy-scheduler`, with two additions
for a service that runs unattended for days:

1. **Waiting is a state.** A run waiting for an answer shows as *waiting for
   you* (with the question, since when, and its time-outs) in `overview`, the
   app and shims.
   - An *Ask me* block's own time-out applies.
   - The job's time limit applies.
   - A per-instance maximum wait (default 7 days) stops runs that would
     otherwise wait forever.
2. **Questions outlive connections, not restarts.** Waiting questions are
   kept in `asks/`, so any client that connects later sees them.
   - A run's progress is held in memory, so if chummyd restarts, its waiting
     runs end as **interrupted**, a new history status.
   - The questions are marked withdrawn, and the app offers to run the job
     again.
   - (Resuming a loop mid-run would need the engine's state to be saved. That
     can come later, if it's needed; the engine's queues and values are
     already plain data.)

### Where questions show up

- **The Chummy app**, when connected: each instance's questions appear in
  the main window's *Waiting for you* list and as chat messages in that
  instance's section (*chummyd: home-server → Researcher*). They're answered
  the same way as local ones. The app passes answers over its authenticated
  session.
- **Shims** (bridges to messaging apps, for answering away from your
  computer).

### Shims

A shim is a small separate process that connects to chummyd over the local
socket with its own key and the `asker` role. It relays questions out to a
messaging service and answers back in. It can't see files, config or
replies unless configured to. The interface is generic, so shims can be
written for Matrix, ntfy, email, Signal or Telegram.

**The first shim: Matrix.**

- **The server.** Either run a small homeserver next to chummyd
  (conduwuit/Conduit, a single Rust binary, or Synapse) and federate with
  your account, or use an existing homeserver. The shim logs in as a bot
  account (`@chummyd-home:example.org`).
- **The room.** It creates (or joins) one end-to-end encrypted room with the
  allowed people, using matrix-sdk with encryption and cross-signing.
- **Each question** becomes one message: who's asking (the job and agent) and
  the question, with a short reference.
  - A question is answered by replying **in its thread**.
  - A tool approval is answered with an explicit `allow` or `deny <reason>`
    reply in the thread. Reactions aren't accepted, to avoid accidental
    taps.
- **Who can answer:**
  - only listed Matrix user IDs, from **verified devices**;
  - only replies to the question's own thread;
  - each question answered once, and nothing after it's withdrawn.

  Everything else is ignored and logged.
- **Less content on request.** For channels that aren't end-to-end
  encrypted, a shim can send only "Researcher has a question. Open Chummy to
  answer.", with no question text.
- **Tool approvals over shims are off by default per agent**
  (`shims_may_approve_tools`). A remote "allow" of a web request is exactly
  what a prompt-injection attack would want, so it has to be chosen on
  purpose, agent by agent. Questions from *Ask me* blocks can always be
  answered.

## Connecting the Chummy app to chummyd

- **Settings → Background services** lists paired instances.
  - **Add** asks for an instance: "On this computer" finds the local socket
    of a base folder you pick; "On another computer" takes an address and
    certificate fingerprint. Then it asks for a pairing code from
    `chummyd pair`.
  - The app's key is made and kept in the OS keychain; the webview never
    sees it.
- **All calls go through Rust.** The app's Rust side holds the connections
  and forwards calls and events to windows, just like local commands. The
  webview never opens sockets.
- **The Scheduler window gets a source switch:** *This computer* and each
  instance. Both use the same views (job list, form, run history, live runs,
  questions), because chummyd returns the same types the local scheduler
  does. Editing a job on an instance needs `admin`; running and stopping
  need `operator`. The UI hides what the role can't do, and chummyd checks
  anyway.
- **Live runs** from an instance appear in the Sessions panel under the
  instance name, and can be stopped there.
- **Moving jobs:**
  - **Send to chummyd…** on a job exports it (with its loop, if needed) to
    an instance, mapping its agent to one of the instance's agents.
  - **Bring back** copies a job from an instance into the app, switched off.

## Hardening checklist

**Code and supply chain**

- [ ] All Rust, with `unsafe_code = "forbid"` (as the workspace already has).
- [ ] `cargo-deny` (advisories, licenses, sources) and `cargo-audit` in CI.
- [ ] Fuzz the parsers: loop, job, config, IPC frames, tool headers.
- [ ] Reproducible release builds, an SBOM, signed binaries and packages,
      published checksums.

**The process**

- [ ] systemd unit with:
  - `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=yes`,
    `ReadWritePaths=<base>`;
  - `PrivateTmp`, `PrivateDevices`, an empty `CapabilityBoundingSet`;
  - `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`, `SystemCallFilter=@system-service`;
  - `LockPersonality`, `RestrictNamespaces`.

  `MemoryDenyWriteExecute` is left out, because Wasmtime compiles code at
  run time.
- [ ] macOS launchd daemon with its own user and a minimal sandbox profile;
      Windows virtual service account with a restricted token.
- [ ] Refuses to start as root or Administrator, with a world-writable base
      folder, or with a config or secret file readable by others.

**Runs**

- [ ] Every limit from the app: loop steps, model and tool calls, tool memory
      and time, response sizes, job rates, file-run rates.
- [ ] Plus chummyd's own: concurrent runs, queue length, a daily model-call
      budget, disk quota per jail, and history and journal retention.
- [ ] Online tools ask first by default; tool approvals over shims are off by
      default.
- [ ] The built-in engine is optional, listens on 127.0.0.1 only, picks its
      own port, uses a random token, and runs offline.

**Connections**

- [ ] Local socket or pipe with peer checks.
- [ ] Remote access only over TLS 1.3 with pinned certificates.
- [ ] ed25519 challenge-response authentication; no bearer tokens.
- [ ] Roles, scoped per agent.
- [ ] Rate and size limits; idle time-outs.
- [ ] Constant-time comparisons; no secrets in responses or logs.

**Records**

- [ ] Hash-chained audit log of every authenticated action and every config
      change.
- [ ] Run journals with each tool call and each web request a tool made.
- [ ] `chummyd export-audit` for review.

**Defaults:** remote access off, executor off, shims off, no paired clients,
every tool off, internet off.

## Code reuse and what to prepare first

Already shared (built in pass 5 with chummyd in mind):

| Crate | What chummyd uses |
|-------|-------------------|
| `chummy-flow` | The loop format, validation and engine (including asking and tool confirmation). |
| `chummy-runner` | `AgentHost` (models, tools, jail, questions, permissions) and `run_job`. chummyd implements `Services` (secrets from files or env, its tool host, its engine). |
| `chummy-scheduler` | `Job` format, triggers, `Planner`, `FolderWatch`, `JobStore`, `Manager`, `AskBroker`. chummyd implements `JobRunner` (as the app does with `AppJobs`). |
| `chummy-tools`, `chummy-sandbox` | The WebAssembly tool sandbox and the folder jail. |
| `chummy-llm` | Model connections, including the https rule for remote servers. |
| `chummy-store` | Atomic, versioned JSON files. |
| `chummy-local` | Optional: the built-in engine. |

To prepare before chummyd's first milestone:

1. Move the loop library (`src-tauri/src/library.rs`) into a crate, so both
   programs share it.
2. Move `Agent`, `Permissions` and `ToolPref` out of the app's settings into a
   shared crate, and add per-agent tool lists (which the app should get
   too; see `docs/security.md`).
3. Add an **interrupted** status to `RunRecord`, and a hook in `AskBroker` to
   persist and restore waiting questions.
4. Add a run journal writer (each run's events to a file) behind a trait in
   `chummy-runner`.
5. Put the IPC message types in their own crate (`chummy-protocol`), with
   the version handshake, so the app and chummyd build against the same
   definitions.

## Milestones

1. **M0, preparation:** the refactors above, plus per-agent tool lists in
   the app.
2. **M1, headless runner:**
   - `chummyd init|check|run|status`;
   - the base folder and the master config;
   - jobs running from config, history in `jobs/`;
   - a local socket with peer checks, read-only (`viewer`).
3. **M2, the app connects:**
   - pairing, keys and roles;
   - the Scheduler's source switch, live runs, and running and stopping jobs
     (`operator`);
   - Export for chummyd (writing a bundle).
4. **M3, questions:**
   - questions answered from the app;
   - waiting questions kept in `asks/`, and the *interrupted* status;
   - the audit log.
5. **M4, remote and admin:**
   - remote TLS with pinned certificates;
   - `config.apply` with diffs, and pushing bundles from the app.
6. **M5, the Matrix shim:** E2EE room, threads, verified devices only,
   approvals off by default.
7. **M6, the executor API:** `executor.run`, with roles and agent scoping.
8. **M7, packaging and service hardening:** the hardening checklist, signing
   and release.

## Open questions

- Should a stopped or restarted chummyd be able to **resume** a waiting run
  (saving the engine's state), or is "interrupted, run again" enough?
- Which messaging bridges matter after Matrix (ntfy is the simplest; Signal
  is the most private)?
- Should a bundle be able to carry API keys encrypted to the instance's
  public key, for one-step setup, or should keys always be entered on the
  server?
- Is a per-instance daily budget in model calls enough, or are budgets in
  money (per connection, with prices entered by the user) needed?
