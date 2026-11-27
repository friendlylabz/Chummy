# Chummy

> [!WARNING]
> **Chummy is a heavy work in progress, and 100% vibe-coded:** every line was
> written by an AI (Claude) at a person's direction. Expect rough edges, bugs
> and breaking changes between versions. Try it on folders you don't mind an
> agent changing, and keep backups.

A minimalist, configurable agent harness for Windows, macOS and Linux.

## Download

Get the latest build from the [Releases page](https://github.com/friendlylabz/Chummy/releases):

| System | File | Notes |
|---|---|---|
| **Windows** | `Chummy_…_x64-setup.exe` | Windows may say it "protected your PC" because the app isn't signed yet: click **More info → Run anyway**. (There's also an `.msi`.) |
| **macOS** (Apple Silicon: M1 and newer) | `Chummy_…_aarch64.dmg` | Open it and drag Chummy to Applications. The app isn't notarized yet, so the first time, macOS will refuse to open it: go to **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** (or right-click the app → Open). |
| **macOS** (Intel) | `Chummy_…_x64.dmg` | As above. |
| **Linux** | `Chummy_…_amd64.AppImage` | Make it runnable (`chmod +x Chummy_*.AppImage`, or Properties → Permissions) and open it. Some distributions need `libfuse2` for AppImages. There are also `.deb` and `.rpm` packages; on Debian and Ubuntu the `.deb` is the better fit. Needs Ubuntu 24.04, Debian 13, Fedora 40 or newer. |

The first time it opens, Chummy walks you through connecting a model (for
example [Ollama](https://ollama.com) or [LM Studio](https://lmstudio.ai) on
your computer, or a cloud provider), adding the recommended loop and setting
up an agent with its own folder.

Chummy assumes the model behind it may be small and easily confused. Agent
loops are built from small, editable blocks in a visual editor; every agent is
locked to one folder you choose; and nothing reaches the network or runs a tool
until you say so.

## Documentation

- [Using Chummy](docs/user/README.md): [getting started](docs/user/getting-started.md),
  [loops and the loop editor](docs/user/loop-editor.md), [settings](docs/user/settings.md),
  [chummyd](docs/user/chummyd.md)
- [Developing Chummy](docs/dev/README.md): [modules](docs/dev/modules.md),
  [security and threat model](docs/dev/security.md), [file formats](docs/dev/file-formats.md),
  [roadmap](docs/dev/roadmap.md)

## Features

- **A visual editor for the agent loop itself.** How an agent thinks isn't
  hidden in code: it's a graph of blocks you can open, read and change,
  ComfyUI-style. Blocks ask the model, run tools, ask you, branch on
  patterns, keep notes in memory, build and trim the conversation, and
  summarise it when it gets long. Loops can cycle (draft, check, retry)
  within limits you set, groups of blocks become reusable **functions**,
  and comments, link labels and notes are saved with the loop so it
  explains itself. Test runs light up each block as it works. Loops are
  plain, versioned JSON you can export and share.
- **Ready-made loops you can take apart.** The **Chummy assistant** is a
  full agent loop like a coding agent's: its own to-do list, `CHUMMY.md`
  instructions, context compaction, a decided next step before every call,
  and failures reported and retried. It comes in three sizes: **small**
  (more steering, a check of each answer, asking when unsure, a light
  context), **medium** (the default) and **large** (no steering: it plans
  and does the job in one go, and uses its context to remember). All are
  built from ordinary blocks you can change.
- **Built for small models.** Every model block can switch tools off,
  steer the model with a reminder sent last, fall back to tool calls
  described in the prompt when a server can't call tools, and explain bad
  calls and ask again. Failures go out of a block's *Failed* output, so the
  loop can handle them instead of dying.
- **Per-agent tuning without forking the loop.** Agent Loop Configuration
  changes any block's settings for one agent; model settings (temperature,
  reasoning effort, sampling) can be overridden per agent.
- **See everything that happened.** Under each reply, *Agent logs* shows
  the model's reasoning, tool calls and every block's inputs and outputs,
  live while it runs. A run's pane shows what the loop reports as it goes,
  the sessions panel keeps each agent's three latest runs with their pane
  and log, and the Logs window searches every run.
- **Talk to agents from your phone.** Settings → Channels connects an agent
  to end-to-end encrypted Matrix rooms (with a bot account you choose) or a
  Telegram bot, the same channels chummyd has and the same code. People you
  allow verify the Matrix bot by emoji, or pair with the Telegram bot by a
  code you make; their messages start runs that show in the sessions panel
  like any other.
- **Any model.** Ollama, LM Studio, llama.cpp, vLLM, any OpenAI-compatible
  server, OpenAI, Anthropic, Gemini, OpenRouter and more, plus a basic
  built-in engine for Hugging Face models. Daily token limits per model.
- **Scheduled jobs.** Run an agent at set times or when files land in its
  folder, with the same loop, folder and permissions as in chat.

## Security

- **Every agent is jailed to one folder** you pick and confirm. Drives, your
  home folder, system and settings folders and Chummy's own folder are
  refused, and file access goes through a capability handle that symlinks
  can't escape.
- **Tools are sandboxed, always.** Choose where they run: **Native**
  (CPython for WebAssembly, the default, nothing to install), **Container**
  (a fresh, locked-down Docker or Podman container per call) or **NVIDIA
  OpenShell**. There is no unsandboxed mode.
- **Tools are off until you turn them on**, and so is internet access. Each
  agent has its own tool switchboard (on, ask first, internet, per tool),
  under the app-wide switches; a loop block can only switch tools off.
  Turning things on asks through a native confirmation, and *potentially
  unsafe* tools (like the command line) ask before each use.
- **One simple tool format.** A tool is one Python file with a standard
  header. It sees only the agent's folder, has no network of its own (web
  access goes through Chummy, only when allowed), and its dependencies are
  pure Python from a checksummed lock file.
- **Loops can't do anything on their own.** There's no script block: blocks
  only handle text, call models and work inside the agent's folder. Your
  message coming in, tools and output going out are the only ways a loop
  touches the world.
- **Loops from elsewhere are reviewed first.** Only loops on a signed list
  of trusted loops run: the built-in ones, the ones you make, and the ones
  you've reviewed. Anything else opens a review that shows what the loop can
  do, every prompt it sends, where its data flows, and flags for risky
  patterns, before you decide.
- **Secrets stay in the system keychain**, never in files. **Run logs are
  encrypted** (XChaCha20-Poly1305, key in the keychain, exportable under a
  passphrase), and the system log never records message text, files or keys.

See [security and threat model](docs/dev/security.md) for the threat model, the sandboxes and the audits.

## License

[MIT](LICENSE) © 2026 friendlylabz. Chummy includes CPython built for
WebAssembly (PSF License; its LICENSE file is unpacked with it). The built-in
engine downloads third-party open-source software at runtime (uv, CPython,
PyTorch, transformers and their dependencies) under their own licenses, and
two built-in tools download a package each from PyPI at setup.
