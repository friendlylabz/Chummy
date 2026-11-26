# Getting started

## Install

Download the latest build from the
[Releases page](https://github.com/friendlylabz/Chummy/releases).

- **Windows:** run `Chummy_…_x64-setup.exe`. If Windows says it "protected
  your PC" (the app isn't signed yet), click **More info → Run anyway**.
- **macOS:** open the `.dmg` (`aarch64` for Apple Silicon, `x64` for Intel)
  and drag Chummy to Applications. The first time, macOS refuses to open
  it: go to **System Settings → Privacy & Security** and click **Open
  Anyway**.
- **Linux:** make the `.AppImage` runnable (`chmod +x`) and open it, or
  install the `.deb` or `.rpm` (on Debian and Ubuntu, the `.deb` is the
  better fit). Some systems need `libfuse2` for AppImages. Chummy needs
  Ubuntu 24.04, Debian 13, Fedora 40 or newer.

## First run

The welcome screen walks you through three steps:

1. **Connect a model.** A model on your computer (Ollama, LM Studio,
   llama.cpp, vLLM, the built-in engine), on your own server (any
   OpenAI-compatible address), or a cloud provider (OpenAI, Anthropic,
   Gemini and others). Click **Test** to check it replies, can call tools,
   and how long its context is.
2. **Add a loop.** **Use recommended** adds the Chummy assistant, a good
   general-purpose loop. You can change it or build your own later.
3. **Set up an agent.** Give it a name, a loop, a model and a **workspace
   folder**. The agent can only ever see that folder, so pick (or make) one
   just for it. Don't use a folder with things you can't afford to lose.

Two optional steps let you look at **Security and Privacy** and **Tools**
now or later.

## Turning on tools

New agents start with no tools and no internet. To let an agent read and
write files, run commands or fetch pages:

1. Settings → **Tools**: click **Set up tools**, then switch on the tools you
   want. Each asks before it turns on.
2. Settings → **Security and Privacy**: turn on **Tools** (and **Internet
   access** if a tool needs the web).
3. Settings → **Agents** → your agent: turn tools (and internet) on for
   that agent.

A tool runs only when it's on at every level. Tools marked *Potentially
unsafe* ask before each use unless you change that.

## Chatting

- Type in the message box and press Enter. The agent's loop runs inside
  its folder; while it works you see the current step and a **timer**
  (minutes, seconds and tenths), and you can **Pause** or **Stop** it.
- **Pause** (next to Stop, in the message box and in the Sessions panel)
  lets the run finish the step it's on, then keeps everything it has
  done so far. Resume it whenever you like, even after quitting Chummy or
  restarting your computer: the message box and the Sessions panel's
  **Paused** list show **Resume** (and **Discard**, to throw it away), and
  so does the paused note in the response itself. Resuming removes the
  note and carries on in the same response, as if it had never stopped:
  its time and figures count the whole run, and Agent logs show one log. If
  it was waiting for an answer from you, it pauses straight away and asks
  again when resumed. While one chat is paused you can use others, with
  any agent. If the model can't be reached (the server is off, the
  connection dropped, it's overloaded), a chat **pauses on its own**
  instead of ending, so nothing is lost: resume once the model is back.
- Each message you send gets **one response**, built as the run goes: what
  the agent says along the way, any question it asked with your answer,
  and how it ended. Under it: how long the run took, then model calls, tool
  calls, steps, tokens used, how full the context is, and today's use of
  the model's daily limit.
- **Agent logs** opens the run's log right there: model answers and
  reasoning, tool calls and results, questions and notices, **newest at
  the top** (switch to *In order* to read it from the start). Block steps
  are hidden at first; click the **Block steps** chip to see every block
  the loop ran. **Open in Logs** shows the same run in the Logs window.
- If an agent asks you something, answer in the message box. Tool
  approvals show as a card with **Allow** / **Don't allow**.
- In the **Sessions** panel, each running chat has a text button: click it
  to see the run's **pane**, a live list of what its loop has reported
  (the Chummy assistant says what it's doing next before each step),
  newest at the top. The × closes it. Drag the panel's left edge to make
  it wider or narrower (double-click the edge to reset); the chat grows
  with the window.
- The chat picker (next to the agent picker) starts a **New chat** or opens
  an earlier one. An agent's chats can run side by side.
- The trash can under a message, or under a whole response, deletes it
  (the agent won't see it either); **Undo** brings it back.
- In the message box, **Ctrl+Z** (Cmd+Z on a Mac) undoes typing a word at a
  time, and **Ctrl+Shift+Z** or **Ctrl+Y** redoes.

## Your instructions for a folder

Put a `CHUMMY.md` file at the top of an agent's folder with anything it
should always know (what the folder is for, how you like things done). The
Chummy assistant reads it at the start of every message.

## Where Chummy keeps things

| System  | Chummy folder                                           |
|---------|---------------------------------------------------------|
| Linux   | `~/.local/share/com.friendlylabz.chummy`                |
| macOS   | `~/Library/Application Support/com.friendlylabz.chummy` |
| Windows | `%LOCALAPPDATA%\com.friendlylabz.chummy`                |

It holds `settings.json` (versioned, written atomically, with a `.bak`
backup), `loops/` (your loops, one JSON file each), `trusted-loops.json` (the
loops that may run without a review, signed with a key in your keychain), `chats/` (a folder per
agent with an index and one file per chat), `jobs/` (scheduled jobs, their
state and run history), `usage.json` (tokens used per model per day, kept
31 days), `logs/system/` (the system log, one file a day), `logs/runs/` (one
log per run, by day), `models/` and `runtime/` (the built-in engine) and
`tools/` (the WebAssembly Python and installed tools).

Chummy keeps the folder private to your user account: on macOS and Linux it
makes the folder 700 and everything in it 600 (keeping the owner's execute
bit for programs), and checks again every time it starts. On Windows the
default folder is inside your user profile, which is already private to you.

To use another folder, start Chummy with `--data-dir <folder>` (or
`--data-dir=<folder>`); a relative path is taken from the current folder. It
must be empty, new, or an existing Chummy folder, so Chummy never takes over
a folder with other things in it. Settings → Security and Privacy shows which
folder is in use.

API keys and other secrets are never stored here: they go to your system
keychain (Keychain on macOS, Credential Manager on Windows, Secret Service on
Linux). Set `HF_ENDPOINT` to use a Hugging Face mirror for the built-in
engine's downloads.
