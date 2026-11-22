# Chummy

A minimalist, configurable agent harness for Windows, macOS and Linux.

Chummy assumes the model behind it may be small and easily confused. Agent
loops are built from small, editable blocks in a visual editor; every agent is
locked to one folder you choose; and nothing reaches the network or runs a tool
until you say so.

![Chatting with an agent](docs/screenshot-main.png)

> **Status:** passes 1–7 of the roadmap are done: the app shell, the folder
> jail, settings, model connections (including a basic built-in engine), the
> loop engine and editor, agents and chat, sandboxed Python tools, exchanges,
> block functions, the scheduler with tray mode and questions for you, and
> the default **Chummy assistant** loop with tool modes, working notes,
> failure outputs, daily token limits and per-agent settings, and logs of
> every run (each block's inputs and outputs) plus a system log, viewable in
> the app and under each chat message. Next: chummyd,
> an optional background service ([plan](docs/chummyd-plan.md)).

![The 5 step recursive plan answering in chat, using tools](docs/screenshot-plan-chat.png)

## What works today

- **Loops.** A loop is a directed graph of blocks that pass text along links,
  from your message to the agent's reply. A block waits until every input has
  a value, runs, and sends its result to everything linked to its outputs.
  Cycles are allowed (with step and model-call limits), so a loop can draft,
  check its own answer, and retry. The central block is **Ask the model**: a
  prompt of your own with `_INPUT_` where the block's input goes. Loops are
  plain JSON in a strict, versioned format: see
  [docs/loop-format.md](docs/loop-format.md).
- **Loop editor.** A node editor with a block palette, an inspector for every
  parameter, live checks, templates, import and export, and test runs that
  light up each block as it works. Blocks can carry a **comment**, links a
  **label**, and the canvas **notes**, all saved in the loop file so a loop
  explains itself.
- **Tools.** A tool is one Python file with a standard PEP 723 header. It runs
  as WebAssembly (CPython for WASI, under Wasmtime) and can see only the
  agent's workspace. It can't go online except through Chummy, and only if
  it asks for that, Internet access is on, and you allowed that tool. There
  is no unsandboxed mode. Dependencies come from a checksummed PEP 751 lock
  and must be pure Python. Seven built-in tools ship with Chummy: read,
  write, list and search files, calculator, current time, and fetch web page.
  See [docs/tool-format.md](docs/tool-format.md).
- **Model block with tools.** Ask the model has outputs **Answer**,
  **Tool call** (fires instead of Answer when the model calls a tool),
  **Reasoning** and **Failed**. Each block picks its own tools, which keeps
  prompts short for small models. **Tool handling** is *Native* (the
  server's tool calling, the default), *Prompted* (tools described in the
  system message and calls parsed from the text, for models or servers
  without tool calling) or *Off* (the loop handles tools itself with
  `_TOOLS_` in a prompt and a **Find tool call** block). Bad calls (unknown tool, wrong arguments, no call when
  one is required) can be explained to the model and retried, stop the run,
  or fall back to the text. A **Run tool** block runs the calls and passes
  the results back.
- **Failures you can route.** Blocks that can fail (model, Run tool, Ask me
  and the file blocks) have a **Failed** output carrying the error message.
  Link it and the loop carries on; leave it unlinked and the failure stops
  the run as before.
- **Working notes.** **Save to memory** and **Read memory** blocks keep text
  under a name for the rest of the run, and any prompt can include it with
  `{{name}}`. The assistant uses this to keep notes that every model call
  sees and that the model updates after each step.
- **Daily token limits.** Each model connection has a daily limit (1,000,000
  tokens by default, or off), shown with today's use in Settings → Models.
  When it runs out, each model block waits until midnight, stops the run, or
  asks you for more, as you choose.
- **Logs.** Every run of a loop (chat, job or test) is logged: each block
  it executed with its inputs and outputs (functions included, nested),
  every model call with what was sent, the reasoning and the answer, and
  every tool call, question, notice and reply. Chummy also keeps a system
  log of what it did: starts, settings and permission changes, runs, jobs,
  tools, questions and errors (never message text, files or keys). The
  **Logs** window (main menu → Logs) finds runs by text, kind, status, agent
  and date, searches inside logs, and sorts them; within a run you can
  search, show or hide each kind of entry, pick block types, show only
  failures, and sort by order or time taken. Logs are kept for 30 days.
- **What happened, under each reply.** Every reply in chat has a collapsed
  **What happened** view of that run: the model's reasoning and answers,
  tool calls and questions, and (one click) every block step, with the same
  search and filters. It follows a run live while it works.
- **Exchanges.** A conversation with the model that flows between blocks:
  *Start exchange*, *Add to exchange* and *Read exchange* build and read it,
  and model blocks continue it.
- **Block functions.** Select some blocks and choose **Make function**: they
  become one block whose ports are exactly the inputs and outputs you left
  open. Double-click to edit inside. Functions are saved in the loop file.
- **Chummy assistant (the default).** Asks you one question if your request
  is unclear, tries it with tools (the model checks each call before it
  runs), breaks anything too big into steps, checks every result and plan,
  keeps working notes, asks you when it's stuck, and carries on past
  failures. Each result is shown as it's done, and each breakdown shows
  **the whole plan**, nested by level with finished tasks ticked (a **Plan
  outline** block keeps it, and the model sees it as `{{plan}}`). Onboarding offers it with one click, and a first agent gets it
  automatically.
- **Templates.** Chummy assistant, Blank, Simple chat, Draft and self-check, **5 step
  recursive plan** and **5 step recursive plan verify**. The planner tries
  your goal straight away (by writing or with tools); anything too big is
  broken into 5 or fewer steps, and each step is tried the same way,
  breadth-first, with every result and plan shown as it goes. The verify
  version also has the model write a yes/no check question for each result
  and plan and answer it, breaking down results that fail and re-planning
  plans that fail.
- **Scheduled jobs.** A job runs one of your agents with a message: at set
  times, every so often, once, or when files land in a folder of the
  agent's. It runs with exactly that agent's folder, model and permissions.
  Jobs can include a file, save their reply to a file, have a time limit,
  catch up on times missed while Chummy was closed, and keep a history of
  their runs. See [docs/job-format.md](docs/job-format.md).
- **Questions for you.** An **Ask me** block asks you something in chat and
  waits for your answer; a **Run tool** block can wait for your OK before
  each call. Scheduled jobs ask in the agent's chat and the Scheduler, and
  Chummy sends a notification. Tools that can go online always ask first,
  unless you turn that off.
- **Tray mode.** Chummy's icon stays in the tray while it runs. Closing the
  window asks whether to quit (stopping everything) or keep running in the
  tray so jobs carry on; you can make either the default.
- **Agents.** An agent is a loop, a model, and **its own workspace folder**.
  Each folder belongs to exactly one agent, and no agent's folder can be inside
  another's. Chat with an agent from the main window; its loop runs inside its
  folder, and progress shows live. Each agent has its own **tool and internet
  switches**, and **Agent Loop Configuration** changes any block's settings
  for that agent alone, without touching the loop file. Those settings can be
  exported and imported as `chummy.loopconfig` files.
- **Workspace jail.** Each agent is locked to its folder, which you pick and
  confirm in a native dialog. Whole drives, your home folder, system folders, hidden
  settings folders (`.ssh`, `.config`, …) and Chummy's own folder are always
  refused, along with any folder that contains one of them. File access goes
  through a `cap-std` capability handle, so symlinks and junctions can't
  escape the folder.
- **Permissions.** Internet access and tools are off by default, and so is
  every tool, and every tool's internet access. Turning any of them on needs
  a native confirmation, shown by Rust, not by the web UI. A tool runs only
  when it's on at every level: app-wide, for the agent, and in the block. See
  [docs/security.md](docs/security.md) for the threat model and the latest
  audit.
- **Model connections.** Ollama, LM Studio, llama.cpp, vLLM, any
  OpenAI-compatible server, OpenAI, Anthropic, Gemini, OpenRouter, Groq,
  Mistral, DeepSeek, xAI and Together. API keys live in the OS keychain. **Test
  connection** checks that the server is reachable, that the model replies,
  and whether it can call a tool.
- **Built-in engine (basic).** Paste a Hugging Face repo and Chummy checks it
  before downloading: it must be a text model, run without custom code, ship
  safetensors weights, and have a chat template with tools in a format Chummy
  can parse. The check shows the download size and estimated memory needs on
  common setups and on this computer. Downloads resume and are checksummed.
  Python, PyTorch (with the right CUDA, Metal or CPU build) and transformers
  are installed on first use with [uv](https://github.com/astral-sh/uv), inside
  Chummy's folder; none of them ship with the app. The engine is much slower
  than vLLM or llama.cpp, and the app says so.

![The Chummy assistant in the loop editor, with comments, link labels and notes](docs/screenshot-assistant.png)

![Agent Loop Configuration: a block's setting changed for one agent](docs/screenshot-agent-loop-config.png)

![The Logs window: a run's blocks with their inputs and outputs](docs/screenshot-logs.png)

![A block function open in the editor: the model block and Run tool in a cycle](docs/screenshot-function.png)

<p>
  <img src="docs/screenshot-ask.png" width="49%" alt="A scheduled job asking to run a tool, in the agent's chat">
  <img src="docs/screenshot-scheduler.png" width="49%" alt="The Scheduler: a job with a one-off time and weekday mornings">
</p>

<p>
  <img src="docs/screenshot-tools.png" width="49%" alt="Settings: built-in tools, each off until switched on">
  <img src="docs/screenshot-settings-models.png" width="49%" alt="Model connections">
</p>

## Where Chummy keeps things

| OS      | Chummy folder                                           |
|---------|---------------------------------------------------------|
| Linux   | `~/.local/share/com.friendlylabz.chummy`                |
| macOS   | `~/Library/Application Support/com.friendlylabz.chummy` |
| Windows | `%LOCALAPPDATA%\com.friendlylabz.chummy`                |

It holds `settings.json` (versioned, written atomically, with a `.bak`
backup), `loops/` (your loops, one JSON file each), `chats/` (chat history,
one file per agent), `jobs/` (scheduled jobs, their state and run history),
`usage.json` (tokens used per model per day, kept 31 days), `logs/system/`
(the system log, one file a day) and `logs/runs/` (one log per run, by day),
`models/`,
`runtime/` (the built-in engine), `tools/` (the WebAssembly Python
and installed tools) and `logs/`.
Secrets are never stored here; they go to the system keychain (Keychain,
Credential Manager, or Secret Service on Linux). Set `HF_ENDPOINT` to use a
Hugging Face mirror.

## Development

**Prerequisites**

- [Rust](https://rustup.rs). The version is pinned in `rust-toolchain.toml`, and rustup installs it automatically.
- Node.js 22+
- Platform libraries:
  - **Linux:** `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev libdbus-1-dev build-essential`. Saving API keys at runtime needs a Secret Service provider (GNOME Keyring, KWallet).
  - **macOS:** Xcode Command Line Tools (`xcode-select --install`)
  - **Windows:** Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 10/11)

**Run**

```sh
npm install
npm run tauri dev      # app with hot reload
npm run dev            # UI only, in a browser (IPC falls back to first-launch defaults)
```

**Build installers**

```sh
npm run tauri build    # MSI/NSIS · .dmg/.app · .deb/AppImage/.rpm
```

**Checks**

```sh
npm run check                                          # Svelte + TypeScript
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                 # includes an end-to-end engine test (needs python on PATH)
python3 -m unittest discover -s crates/local/engine    # engine parsers and server
```

The sandbox tests run real tools on the real WebAssembly Python. Point them at
the archive (CI downloads it), and allow PyPI downloads for the tools with
dependencies:

```sh
curl -LO https://github.com/brettcannon/cpython-wasi-build/releases/download/v3.14.7/python-3.14.7-wasi_sdk-24.zip
CHUMMY_TEST_PYTHON_ZIP=$PWD/python-3.14.7-wasi_sdk-24.zip CHUMMY_TEST_PYPI=1 cargo test -p chummy-tools
```

Pushing a tag like `v0.1.0` runs `.github/workflows/release.yml`, which builds
installers for all platforms and attaches them to a draft GitHub release.

## Architecture

```
crates/          Pure Rust libraries, no Tauri dependency, tested on their own
  sandbox/       Workspace validation, protected locations, cap-std jail, grants
  llm/           Provider configs, adapters (OpenAI-compatible, Anthropic, Gemini), probe
  local/         Built-in engine: HF inspection, estimates, downloads, runtime, process
    engine/      chummy_engine.py: the OpenAI-compatible transformers server
  store/         Chummy folder layout, atomic versioned JSON, keychain secrets
  flow/          Loop format, block catalog, validation, the loop engine, templates
    templates/   Template loop files (the 5 step recursive plan)
  tools/         Python tools as WebAssembly: format, locks, installer, sandbox host
    builtin/     The built-in tools and their pylock files
    support/     The in-sandbox runner and the `chummy` helper module
  scheduler/     Jobs, triggers, the run planner, folder watching, history, questions
  runner/        Runs an agent: models, tools, jail, questions; shared with chummyd
  log/           The system log and run logs (JSON lines), search and retention
src-tauri/       Thin app shell: state, IPC commands, native dialogs, windows
  capabilities/  One permission file per window
ui/              Svelte 5 frontend, one Vite page per window
  index.html     Main window: chat, menu, sessions panel
  editor.html    Loop editor (xyflow)
  settings.html  Settings
  scheduler.html Scheduler: jobs, schedules, runs
  logs.html      Logs: run logs and the system log
  src/lib/ipc.ts The only file that talks to Rust
```

**Loop pipeline:** chat message → `send_message` → the agent's loop file is
checked → its workspace is opened as a `Jail` → `chummy_flow::run` executes the
blocks, calling models through the model pipeline and touching files only
through the jail → `run:*` events stream to every window → the replies are
saved to the agent's chat history. Block ports come from the Rust catalog; the
editor asks Rust for them (`analyze_loop`), so there's a single source of truth.

**Job pipeline:** `chummy_scheduler::Manager` checks schedules and watched
folders every few seconds, queues what's due (one run per job and per agent,
missed times skipped or caught up once), and starts runs through a
`JobRunner`. The app's runner opens the agent's jail and runs the loop with
`chummy_runner::run_job`, exactly as a chat would. Questions from any run go
through one `AskBroker` and are answered from the app's windows. None of this
depends on Tauri, so chummyd can reuse it.

**Model pipeline:** `ProviderConfig` → `Client` → adapter → server-sent events →
`ChatEvent` (`Text`, `Reasoning`, `ToolCall`, `MalformedToolCall`, `Done`). Tool
calls are fully assembled and parsed before they're emitted. Invalid arguments
come back as `MalformedToolCall` with the raw text, so a verification step can
ask the model to fix them. The built-in engine is just another
OpenAI-compatible endpoint (127.0.0.1, random bearer token, offline).

**Rules of the road**

- Rust is the source of truth and the only place sandbox decisions are made.
  Windows read state through commands and hear about changes through events.
- The UI never sends a filesystem path to Rust for a workspace: Rust shows
  the picker, validates the choice, asks for confirmation, and hands back a
  one-time token that stands for the folder.
- Each window only gets the commands it needs (`src-tauri/capabilities/`).
- Adding a command: implement it in `src-tauri/src/commands/`, register it in
  `lib.rs`, list it in `build.rs`, allow it in the relevant capability files,
  and add a typed wrapper in `ui/src/lib/ipc.ts`.
- Adding a window: add an HTML page in `ui/`, an entry in `vite.config.ts`, a
  variant in `src-tauri/src/windows.rs` and a capability file.
- Settings schema changes: add a field with a default, or bump `VERSION` and add
  a migration step in `src-tauri/src/settings.rs`.
- Themes are sets of CSS variables (`ui/src/theme/`); components use only those
  variables.

## Roadmap

1. **Skeleton & build system.** Done.
2. **Sandbox, settings, models.** Done.
3. **Loop engine & editor.** Done.
4. **Tools, exchanges, functions.** Done.
5. **Scheduler, tray mode, questions for you, security audit.** Done.
6. **Default assistant loop, tool modes, failure outputs, working notes,
   daily token limits, per-agent switches and loop configuration, editor
   comments and notes.** Done.
7. **Logs: run logs with every block's inputs and outputs, the system log,
   the Logs window, step views in chat, and the whole plan in the
   assistant's output.** Done.
8. **chummyd.** An optional, separately installed background service for jobs,
   with an authenticated API, configured from the app. See
   [docs/chummyd-plan.md](docs/chummyd-plan.md).

## License

[MIT](LICENSE) © 2026 friendlylabz. The built-in engine downloads third-party
open-source software at runtime (uv, CPython, PyTorch, transformers and their
dependencies) under their own licenses; none of it is bundled with Chummy.
Tools likewise download the CPython WASI build (PSF license) and their
packages from PyPI at setup.
