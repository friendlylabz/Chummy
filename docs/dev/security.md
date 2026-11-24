# Security features

A short map of the protections and where they live in the code. The threat
model, the reasoning and every audit are in
[../security.md](../security.md); read it before changing anything here.

| Protection | What it does | Where |
|---|---|---|
| **Folder jail** | Each agent sees one folder the user picked and confirmed in a native dialog. Whole drives, the home folder, system and hidden settings folders and Chummy's own folder are always refused. File access goes through a `cap-std` handle, so symlinks, `..` and junctions can't escape. | `crates/sandbox` (`protected.rs`, `jail.rs`, `workspace.rs`); `commands/agents.rs` hands the UI a one-time token, never a path. |
| **Off by default** | Tools, internet access, each tool and each tool's internet access start off. Turning one on shows a native confirmation from Rust. | `commands/agents.rs`, `commands/tools.rs`, `dialogs.rs` |
| **Every level must agree** | A tool runs only if it's on app-wide, for the agent and in the block; going online also needs the app's, the tool's, the agent's and the block's switch. Loops can only narrow this. | `crates/runner` (`AgentHost::tools`, `net_policy`), `crates/flow` (`ToolSwitches`) |
| **Sandboxed tools only** | Tools are Python and run only in a sandbox backend: Native (CPython for WASI under Wasmtime, with memory and time limits), Container (locked-down Docker/Podman, approved image ID, no network) or OpenShell. There is no unsandboxed mode and no script block. | `crates/tools/src/sandbox/`, `docs/sandboxes.md` |
| **Web access through Chummy** | Tools have no network of their own. Requests go through `NetPolicy`: no private addresses, DNS pinned, redirects re-checked, size caps. | `crates/tools/src/net.rs` |
| **Risky tools ask** | Tools marked `risky` (e.g. `run_command`, `run_shell`) show "Potentially unsafe", confirm when switched on and can ask before each use. | tool headers, `commands/tools.rs`, `AskBroker` |
| **One way to ask** | Questions, tool approvals and budget asks from chats, tests and jobs all go through one broker; answers are checked. | `crates/scheduler/src/asks.rs`, `commands/asks.rs` |
| **Secrets in the keychain** | API keys and the log key live only in the OS keychain. URLs with credentials are refused; plain `http://` to a public host warns and asks. | `crates/store/src/secrets.rs`, `crates/llm/src/config.rs`, `commands/providers.rs` |
| **Encrypted run logs** | Run logs are sealed line by line (XChaCha20-Poly1305); the key can be exported in a passphrase-locked file (Argon2id). | `crates/log/src/crypt.rs` |
| **Your model, your call** | Chummy never calls a model outside a loop unless that connection's **Use model within app** is on (off by default). | `src-tauri/src/in_app.rs` |
| **Per-window permissions** | Each window may call only the commands it needs. | `src-tauri/capabilities/*.json` |
| **Private data folder** | The Chummy folder is made private to the user (700/600 on macOS and Linux) at every start. | `crates/store` |
| **Strict files** | Loop, job and config files are strict (`deny_unknown_fields`), versioned and size-limited; imported jobs arrive switched off. | `crates/flow/src/format.rs`, `validate.rs`, `crates/scheduler` |

When a change touches permissions, secrets or untrusted content, think
through both levels (app and agent) and the block override, and add a note
to [../security.md](../security.md).
