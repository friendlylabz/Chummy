# Chummy

A minimalist, configurable agent harness for Windows, macOS and Linux.

Chummy assumes the model behind it may be small and easily confused. Agent
loops are built from small, editable blocks in a visual editor; every agent is
locked to one folder you choose; and nothing reaches the network or runs a tool
until you say so.

![Chummy on first launch](docs/screenshot-main.png)

> **Status:** pass 1 of the roadmap: the app shell, windows and build system.
> See [the architecture plan](#architecture) for what comes next.

## Development

**Prerequisites**

- [Rust](https://rustup.rs). The version is pinned in `rust-toolchain.toml`, and rustup installs it automatically.
- Node.js 22+
- Platform libraries for Tauri:
  - **Linux:** `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential`
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
npm run check                                  # Svelte + TypeScript
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Pushing a tag like `v0.1.0` runs `.github/workflows/release.yml`, which builds
installers for all platforms and attaches them to a draft GitHub release.

## Architecture

```
crates/          Pure Rust libraries, no Tauri dependency, tested on their own
  sandbox/       Folder jail, permission grants, network policy
  llm/           Provider configs + adapters (OpenAI-compatible, Anthropic, Gemini)
  flow/          Loop graph schema (JSON) + interpreter
  tools/         Wasm (WASI) tool host
  scheduler/     Jobs, triggers, input sources
  store/         Internal persistence (settings, library, sessions)
src-tauri/       Thin app shell: state, IPC commands, window management
  capabilities/  One permission file per window
ui/              Svelte 5 frontend, one Vite page per window
  index.html     Main window: chat, menu, sessions panel
  editor.html    Loop editor (xyflow)
  settings.html  Settings
  scheduler.html Scheduler and dispatch
  src/lib/ipc.ts The only file that talks to Rust
```

**Rules of the road**

- Rust is the source of truth and the only place sandbox decisions are made.
  Windows read state through commands and hear about changes through events.
- Each window only gets the commands it needs (`src-tauri/capabilities/`).
- Adding a command: implement it in `src-tauri/src/commands/`, register it in
  `lib.rs`, list it in `build.rs`, allow it in the relevant capability files,
  and add a typed wrapper in `ui/src/lib/ipc.ts`.
- Adding a window: add an HTML page in `ui/`, an entry in `vite.config.ts`, a
  variant in `src-tauri/src/windows.rs` and a capability file.
- Themes are sets of CSS variables (`ui/src/theme/`); components use only those
  variables.

## Roadmap

1. **Skeleton & build system.** (this pass)
2. **Sandbox, settings, models.** Folder jail, deny-by-default grants, the
   Settings window, API keys in the OS keychain, and LLM adapters.
3. **Loop engine & editor.** A versioned loop schema, a block registry, a Rust
   interpreter, and the full editor with JSON import/export. Chat unlocks here.
4. **Tools & scheduler.** A Wasmtime tool runtime, plus scheduled jobs with
   file, web, folder-watch and timer inputs.

## License

[MIT](LICENSE) © 2026 Friendly Labs
