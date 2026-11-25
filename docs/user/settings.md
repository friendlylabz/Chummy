# Settings

Everything is set in the app (main menu → **Settings**); there are no
config files to edit. Anything that widens what an agent can do asks you
first and says what it means.

## Agents

An agent is a **loop**, a **model** and a **workspace folder** (the only
folder it can see). Per agent you can also set:

- **Tools and internet** for this agent (they must also be on app-wide).
- **Model settings**: every generation setting of the model (temperature,
  longest answer, reasoning effort and the rest below) starts as
  **Inherit**. Choose **Set** to give this agent its own value, or **Don't
  send** to leave it to the server. These apply to every model call the
  agent makes; a model block's own setting in the loop (its **Generation
  settings**) still wins.
- **Agent Loop Configuration**: change any block's settings for this agent
  only, without changing the loop. Export and import these as
  `chummy.loopconfig` files.

Agents can share a folder after a warning that they can then affect each
other.

## Models

Your model connections. For each one:

- Server address, API key (stored in the system keychain) and model name.
- **Generation settings**, all in one group under the model name:
  **temperature**, **longest answer** (always sent; used by every model
  block that doesn't set its own), **reasoning effort** (none to high, for
  reasoning models), **top P**, **top K**, **min P**, **presence** and
  **frequency penalty**, **repeat penalty**, **seed** and up to four **stop
  sequences**. Empty means not sent, so the server's own default applies.
  Settings a kind of server doesn't accept are greyed out with the reason
  and never sent (for example, OpenAI takes no top K, and Anthropic's
  reasoning isn't supported yet). OpenAI-compatible servers get everything
  you set; they ignore what they don't know. Each agent (below) and each
  model block in a loop can change any of them for itself.
- Under **Context and limits**: **context length** (read from the server when it says) and **Make room
  at** (80% of it by default), where the Chummy assistant summarises a long
  conversation.
- **Daily token limit** (1,000,000 by default, or off). When it runs out a
  block waits until midnight, stops, or asks you for more.
- **Use model within app** (off by default): lets Chummy use this model for
  small jobs of its own, like naming chats. Off, Chummy never calls your
  model outside your loops.
- **Test** checks the connection, a reply, tool calling and context length.

Addresses with a password in them are refused, and plain `http://` to a
server on the internet asks first.

## Local engine

The basic built-in engine: paste a Hugging Face repo, Chummy checks it
(safetensors weights, no custom code, a usable chat template), shows the
size and memory needed, and downloads it. Python and PyTorch are installed
inside the Chummy folder on first use. It's slower than Ollama, vLLM or
llama.cpp.

## Tools

**Set up tools** unpacks the tools' Python (it ships with Chummy) and
installs the built-in tools (two of them fetch one package each from
PyPI); then switch each tool on or off. Each tool shows
whether it can go online and whether it's *Potentially unsafe* (those ask
before each use). Tools the current sandbox can't run are greyed out with
the reason.

## Security and Privacy

- **Internet access** and **Tools**: the app-wide switches (off by
  default).
- **Ask before tools go online** (on by default).
- **Sandbox**: where tool calls run. **Native** (built in, WebAssembly, the
  default), **Container** (Docker or Podman, a fresh locked-down container
  per call, using the image you approve) or **NVIDIA OpenShell**. See
  [Security and threat model → Sandboxes](../dev/security.md#sandboxes-where-tools-run).
- The Chummy folder in use, and how it's protected.

## Logs

- **Run logs**: everything, shortened, steps only, or off. Kept 30 days.
- **Encrypt run logs** (on by default) with a key in your keychain. Export
  the key to a passphrase-locked file to read the logs on another computer.
- **System log** level, down to off. It records what Chummy did, never
  message text, files or keys.

## General

- **When I close the main window**: ask each time, keep running in the
  tray (so scheduled jobs carry on), or quit.
- **Notifications**, and how many **scheduled runs** may run at once.

## Appearance

- **Theme**: System, Light or Dark.
- **Format replies** (Markdown) and **Show math** (LaTeX), both on by
  default.
