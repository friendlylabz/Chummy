# Roadmap

Ideas we've discussed for Chummy but haven't built yet. Nothing here is
promised or in a set order; it's a record of what's planned and why, so the
reasoning isn't lost. When something here is built, move its description to
the docs for that part of Chummy and take it out of this file.

## Next: more data types and data sources

- **More value types** in the loop editor beyond text, list and exchange:
  number, yes/no, record (JSON, with an optional schema), table, file (a
  reference to a file in the agent's folder), document (PDF, docx, epub),
  web page, image, audio, date/time, event (what started a run) and a blob
  escape hatch. A small fixed set defined in Rust; no user-defined types at
  first.
- **Values remember where they came from**: you, the model, a tool, or the
  outside world. Outside data stays marked as untrusted through text blocks,
  and untrusted text going into a prompt is fenced and labelled. A new
  switch in Security and Privacy (like "ask before tools go online"): while
  untrusted content is in a run, risky and online tools ask first.
- **Big things by reference** (hash, type, size), kept in a run cache or the
  agent's folder, never passed through the graph as bytes.
- **Explicit converters, made easy.** Port kinds still have to match;
  dragging a link between mismatched ports offers to insert the right
  converter. Every type has a "Show as text" converter. Converters that
  need a model (describe an image, transcribe audio) are model blocks that
  take the new input type.
- **Parsers of untrusted formats** run in the sandbox (WebAssembly by
  default) or are pure Rust; never unchecked native code.
- **Sources** (a data input module), defined once and used two ways:
  *Watch*, a job trigger that starts a run with an event, and *Fetch*, a
  block inside a loop. Built-ins: folder or file changed and schedule (both
  exist today), web page changed (through the same internet checks, with a
  selector and normalising so ads and timestamps don't fire it), RSS/Atom,
  later IMAP. No webhooks for now (they'd need a listening port). Custom
  sources would be sandboxed tool scripts with `poll(state)`; Chummy keeps
  the state, drops duplicates by content hash, limits runs (30 an hour per
  job) and caps payloads.

## Sandboxes

- Test OpenShell against a real gateway (so far only against a fake made
  from its CLI's definitions), Podman, and Docker Desktop on macOS and
  Windows.
- A progress bar when downloading a container image, and the image's size
  and date in the approval dialog.
- Maybe a sandbox per agent (today there's one for the whole app, by
  choice).
- Maybe an OS-native process sandbox (Landlock and seccomp, Seatbelt,
  AppContainer), so real programs could run without installing Docker.

## Security

The threats behind these are in
[Security and threat model → Known risks](security.md#known-risks-and-what-to-do-next).

- Encrypt chat history and job history with the run log key.
- Protect `settings.json` against malware running as you: a MAC keyed by a
  keychain secret, with permissions reset to off when it doesn't match.
- Log each web request a tool makes (address, method, status) in the run
  log.
- A one-click "undo this run" that snapshots files before a job changes
  them.
- Show where a model runs ("on this computer", "on your network", "in the
  cloud") wherever a model is chosen.
- On Linux, check for a tray host before offering "Keep running in tray".
- Code signing and notarization, published checksums, and an updater that
  checks signatures.
- Budgets in money for providers with known prices.
- Hard links already inside an agent's folder can point outside it (agents
  can't create them); maybe refuse files with more than one link.
- Linux without a Secret Service: an encrypted file, locked with a
  passphrase, for API keys and the log key.

## Sharing loops safely

Loops from elsewhere are reviewed and then trusted by fingerprint (see
[Security and threat model → Loops from elsewhere](security.md#loops-from-elsewhere-review-and-trusted-loops-after-v012)).
Ideas for later:

- **A curated library of reviewed loops** (the user's idea), like the
  curated tools: loops submitted to a public repository, reviewed by people
  (the scan as a first pass), and listed by fingerprint. Chummy would download
  a loop, check its hash against the list, and show the review with a
  "Reviewed in the Chummy library" badge before trusting it. What it needs to
  be worth more than a hash check:
  - **A signed list.** Sign the list of fingerprints with a key that ships in
    the app (not just HTTPS), so a compromised server or mirror can't add a
    loop to everyone's trust at once.
  - **Revocations.** A signed list of fingerprints found harmful later, which
    Chummy checks and uses to mark installed copies untrusted again.
  - **People to review.** Every submission and every new version (a new hash)
    needs a careful human look; "reviewed" should never read as "safe".
  - **Not the only door.** Hand-shared loops keep the review path, so the
    library is a trust signal, not a gate; and it works offline once a loop is
    downloaded.
  - **Context.** A loop's behaviour depends on the agent's tools and
    permissions; the listing should say which tools it expects.
- **Runtime checks where data leaves**, which also cover prompt injection:
  the untrusted-content switch already planned above; per-agent lists of
  allowed web addresses for web tools; tool approvals that show the full
  request and highlight data that came from files, the chat or your answers.
- **A per-loop permission** (app ∧ agent ∧ loop ∧ block) for tools and
  internet, so an imported loop starts with only what the review showed and
  the user granted.
- **A rehearsal run** of an imported loop against stand-in tools that record
  calls instead of running them, on a sample folder.
- **Author signatures** (ed25519) on shared loops, shown in the review.
- **Agent Loop Configuration edits** and editor test runs aren't reviewed
  (they're the user's own); a hidden-character check while typing prompts
  would still help when people paste text from elsewhere.

## Other ideas

- Try the assistant with real small models (Qwen 3 4B/8B, Llama 3.x 8B
  through Ollama or llama.cpp) and tune its prompts.
- Show each setting's effective default as placeholder text in the
  inspector and in Agent Loop Configuration.
- An official, curated tool library (tools still can't be installed from
  any file).
- Post job replies to the agent's chat.
- Logs: delete or export one run's log from the window, and highlight
  search matches.
- Chat formatting: syntax highlighting for code blocks, copy buttons for
  math, formatted text in the Logs window.
- Built-in engine: test on a real GPU, faster backends.
- Anthropic reasoning (thinking blocks carried through tool use), and
  generation settings for the built-in engine.
- Faster Logs timeline for very long runs.

## chummyd: the background job service

Status: **being built.** M0 (sharing the app's agent code) and M1 (the
headless runner with the terminal channel, and the Configure chummyd window)
are done: see [chummyd](../user/chummyd.md) and the
[file formats](file-formats.md#chummyd). The rest is the plan agreed in
November 2026. This section is the master plan for
building chummyd, and the reference for keeping Chummy's code ready for it.

### What chummyd is

chummyd is a headless program that runs Chummy agents unattended: their
jobs, and conversations with them through **channels** (Matrix first). It's
for people who want agents available when the app is closed, on another
machine, or around the clock.

- **Configured in the app, then static.** The app's **Configure chummyd**
  window (a main menu item) holds the whole configuration and exports it as
  an encrypted **bundle**. chummyd runs exactly that until it stops. Nothing
  at runtime can change it; reconfiguring means exporting again and
  restarting.
- **Channels only, no API.** chummyd listens on nothing. A channel (Matrix,
  later Telegram, and a terminal channel) is the trusted way to talk to its
  agents, and every connection it makes is outgoing.
- **The same code as the app.** chummyd is built from this workspace
  (`crates/chummyd`, not built by default) on the same crates: `flow`,
  `runner`, `agent`, `session`, `scheduler`, `tools`, `sandbox`, `llm`. How
  a conversation behaves (runs, one response per run, pausing, jobs,
  questions) lives in `chummy-session`, so it can't drift between the two.

### Running it

```text
chummyd run --bundle home.chummyd --jail-dir /srv/chummy-jails [--password-file F] [--state DIR]
```

- The password is asked for in the terminal (no echo), or read from
  `--password-file` (in Docker, a secret at `/run/secrets/…`). Never a
  command-line argument; an environment variable is possible but
  discouraged. It's wiped from memory once the key is derived.
- It refuses to run as root or Administrator.
- **Start-up checks**, any of which stops it with an error:
  1. the bundle decrypts and authenticates, and its version is known;
  2. the configuration validates strictly (unknown fields refused,
     versions checked, sizes limited);
  3. the jail dir, the bundle, the password file and the state folder don't
     contain one another (real paths, links resolved), and the jail dir
     passes the same protected-location rules as an agent's folder;
  4. each agent gets `<jail-dir>/<agent>/`, created if missing;
  5. the state folder opens and authenticates for this instance (missing:
     start fresh; present but unreadable: **stop**, with no override; the
     user moves it aside themselves);
  6. an OS lock on the state folder, so one instance uses it.

### The bundle (`.chummyd`)

One file: a header (format, version, instance id, Argon2id settings) and
one JSON document encrypted with XChaCha20-Poly1305. Never a tar archive,
never extracted to disk. It holds:

- instance settings: name, time zone, limits (concurrent runs, daily model
  calls);
- model connections, with their keys;
- agents: loop, model, per-agent tools (each tool's internet access, online
  tools ask first, whether approvals may be answered over a channel: off by
  default);
- the loops, embedded (trusted because the bundle authenticates);
- the jobs;
- channels, with their credentials and allowed users;
- a random **state key** for the state folder.

The app writes it with the same validation chummyd runs at start-up
(`chummy-bundle`, shared), so an exported bundle is one chummyd accepts.
The app warns that anyone with the bundle and its password gets the keys
and credentials in it, and requires a strong password (offering a
generated passphrase).

### The state folder (`<bundle>.state/`, beside the bundle by default)

- `state.sealed`, encrypted with the state key and bound to the instance:
  conversation histories, paused runs, waiting questions, job state and
  history, daily usage, channel positions.
- The Matrix SDK's SQLite stores, their private data encrypted with the
  state key as the passphrase.
- **Saving:** the SDK commits its crypto changes itself; everything else is
  saved when changed (at most every ~30 s), after a run finishes or pauses,
  and on shutdown (temp file, flush, rename, flush the folder).
- **Shutdown** (SIGTERM, Ctrl-C) pauses every run, saves, and exits. A
  model call that would outlast the deadline is cut short at that block,
  which runs again on resuming. Runs paused by a shutdown wait for
  `/resume`.
- **Known limit:** restoring an older copy rolls the state back, Matrix keys
  included (possibly needing a re-verification). Documented, not prevented.

### Channels

- **Matrix (first).** The bot logs in to an account the user made on their
  homeserver; encrypted rooms only. **One room per agent per user**, named
  in the bundle: conversations are one-to-one, even with several allowed
  users. Only allowed users, from verified devices. On first start chummyd
  sets up cross-signing for the bot; the user verifies it from Element with
  the emoji comparison (chummyd prints the emoji in its log), and only
  allowed users may verify.
- **Terminal.** Talk to agents in the terminal chummyd runs in (whoever has
  it already entered the password). Off unless the bundle turns it on.
- **Telegram (later).** Long polling only, never webhooks; allowed user IDs;
  private chats only. Never end-to-end encrypted, which the app says.
- **What the user sees** is the app's chat: each run is one message, edited
  as it goes, with intermediate outputs and the final response, exactly as
  in the app's chat (no logs, no pane output).
- **What a message can do:** start a run of the agent's predefined loop,
  inside its jail and permissions, or give a command. Nothing else changes
  chummyd's state. Commands: `/stop`, `/pause`, `/resume`, `/discard`,
  `/new`, `/status`, `/jobs`, `/run <job>`, `/help`; questions are answered
  by replying in their thread; `/allow` and `/deny <reason>` answer tool
  approvals only where the bundle allows it for that agent (otherwise the
  request is answered "denied: approvals aren't allowed here", and the
  export warns about tools that would always ask).
- Commands are parsed before anything reaches the model, and only incoming
  messages from allowed users count, so an agent's output can never issue
  one. Unknown commands are answered "unknown command"; anyone else is
  ignored and logged. A message while a run is going is refused ("busy:
  /stop or wait").
- **Files** a user sends go into the agent's jail, in `uploads/` (made on
  the first upload). Agents can't send files out.
- **Jobs** report to the agent's room with the user who set them up.

### Runs and limits

Every limit the app has applies, plus the instance's own. Tools run in the
WebAssembly sandbox only (its Python is built into the binary and the
image); no container or OpenShell sandbox, and no built-in model engine
(use Ollama or another OpenAI-compatible server). Logs go to stdout, with
metadata only by default (run ids, statuses, counts); message text only if
the bundle says so.

### Shipping

A binary per OS and a container image; no service installers. An example
compose file mounts the bundle read-only and the jail dir and state folder
as volumes, passes the password as a Docker secret, and runs as a non-root
user with a read-only root filesystem, `cap_drop: ALL`,
`no-new-privileges` and `stop_grace_period: 2m`. A manually started
*Release chummyd* workflow builds them from the same tag as the app.

### Code layout

| Crate | For chummyd |
|-------|-------------|
| `chummy-agent` | Agents, `AgentConfig` (the bundle's configuration implements it), the loop library. |
| `chummy-session` | Conversations, runs, pausing, jobs, questions. chummyd implements `Platform`: the bundle for configuration, the state folder for `ChatStore` and `PausedStore`, channels for updates. |
| `chummy-runner`, `chummy-flow`, `chummy-scheduler` | The engine, the agent host, jobs and the `AskBroker`. |
| `chummy-tools` (`bundled-python`), `chummy-sandbox`, `chummy-llm` | The tool sandbox, the folder jail, model connections. |
| `chummy-bundle` (new) | The bundle format, shared with the app's Configure chummyd window. |
| `chummy-channel` (new) | The `Channel` trait, command parser, run-to-message rendering, allowlists; terminal, and Matrix and Telegram as features. |
| `chummyd` (new) | The binary: CLI, start-up checks, state folder, wiring. Never depends on Tauri (CI checks). |

`default-members` leaves `chummyd` out, so `cargo build` and the app's
builds are unchanged; `cargo build -p chummyd` builds it, and CI tests the
whole workspace, so a change that breaks chummyd fails CI.

### Milestones

1. **M0, shared code (done):** `chummy-agent` and `chummy-session` hold what
   was in the app (agent types and rules, the loop library, chats, the run
   lifecycle, paused runs behind `PausedStore`, jobs, answers); the tools'
   Python moved into `chummy-tools`.
2. **M1, headless runner (done):** `chummy-bundle`, `chummy-channel`, the
   `chummyd` crate, the state folder, start-up checks, graceful shutdown
   (chat runs pause, cutting a model call short with the engine's `hurry`
   flag), jobs from the bundle, the terminal channel, and the Configure
   chummyd window (open a bundle to change it, keeping its instance).
3. **M2:** the Matrix channel.
4. **M3:** the container, release builds, signing and hardening.
5. **M4:** the Telegram channel.

### Open questions

- Is a per-instance daily budget in model calls enough, or are budgets in
  money needed?
- Instance profiles in the app (how a configuration is kept between
  exports) are still to be decided.
