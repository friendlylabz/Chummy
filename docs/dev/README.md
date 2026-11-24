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
