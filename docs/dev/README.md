# Developer documentation

1. [Modules](modules.md): what each crate, the app shell and the UI do, and
   how a message travels through them.
2. [Building and testing](building.md): prerequisites, commands, CI and
   releases.
3. [Security features](security.md): the protections in the code and where
   each one lives.

Two rules shape everything else:

- **Real logic lives in `crates/`**, with no Tauri dependency, so chummyd
  can reuse it. `src-tauri/` is a thin shell; `ui/` talks to Rust only
  through `ui/src/lib/ipc.ts`.
- **Rust decides.** Every permission, path and confirmation check happens in
  Rust. The UI shows things and asks; it never grants.

The file formats are described in [../loop-format.md](../loop-format.md),
[../tool-format.md](../tool-format.md) and
[../job-format.md](../job-format.md).

## Development

(Building and testing has more detail: [building.md](building.md).)

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

Tools run on CPython built for WebAssembly, which ships inside the app.
`scripts/build-wasi-python.sh` builds it from python.org's source release
(checked against its pinned SHA-256 and its Sigstore signature) with pinned
build tools, into `wasi-python/python-wasi.zip`, where the app build picks
it up. It needs Linux x86_64; CI and every release run it. Without it the app
still builds, but can't set up tools. The sandbox tests run real tools on it:

```sh
scripts/build-wasi-python.sh            # about 5 minutes
CHUMMY_TEST_PYTHON_ZIP=$PWD/wasi-python/python-wasi.zip CHUMMY_TEST_PYPI=1 cargo test -p chummy-tools
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
    templates/   The Chummy and Terminal assistants' loop files and their generator (make_templates.py, assistant.py, terminal.py)
  tools/         Python tools: format, locks, installer, sandboxes (Wasm, container, OpenShell)
    builtin/     The built-in tools and their pylock files
    support/     The in-sandbox runner and the `chummy` helper module
  scheduler/     Jobs, triggers, the run planner, folder watching, history, questions
  runner/        Runs an agent: models, tools, jail, questions; shared with chummyd
  log/           The system log and run logs (JSON lines), encryption, search and retention
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
saved to the chat the message came from. Block ports come from the Rust catalog; the
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
