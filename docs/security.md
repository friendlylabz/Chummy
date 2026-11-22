# Security

Chummy is for people who use AI on their own laptops and desktops, many of
whom won't read a settings page closely. So the defaults have to be safe,
the risky switches have to explain themselves, and a mistake should cost as
little as possible. This file is the threat model, what protects against
each threat, and the results of the security audit done with pass 5
(the scheduler).

## What Chummy protects against

| Threat | Protection |
|--------|------------|
| **The model** doing something you didn't intend, because it's small and confused, or because something it read told it to (*prompt injection*: a web page, a file dropped into a watched folder, a document). | Each agent is jailed to one folder you chose. Tools and internet access are off until you turn them on. Tools only run as WebAssembly. Tools that can go online ask you first by default. Loops can only narrow what an agent may do. |
| **Files and loops from elsewhere**: an imported loop or job, a tool, a model download. | Loop and job files are strict and size-limited. Imported jobs arrive switched off. Tools can't be installed from files (only the built-in suite). Model weights must be safetensors, and code from model repos is never run. |
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
   addresses with a user name or key in them are refused. The same check
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

## Pass 6 changes

- **Per-agent tool and internet switches.** A tool runs only when it is on
  app-wide (Permissions and Tools), for the agent, and ticked in the block.
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
[chummyd-plan.md](chummyd-plan.md).
