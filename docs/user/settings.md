# Settings

Everything is set in the app (main menu → **Settings**); there are no
config files to edit. Anything that widens what an agent can do asks you
first and says what it means.

## Agents

An agent is a **loop**, a **model** and a **workspace folder** (the only
folder it can see). Everything below can be set while you create it, before
**Create agent**, or changed later. Per agent you can also set:

- **Tools**: the agent's switchboard. For each installed tool: **On**,
  **Ask first** (every use waits for your OK in chat; *potentially unsafe*
  tools start with it on) and **Internet** (for tools that use it). Tools
  also need **Tools** on in Security and Privacy, and going online needs
  **Internet access** there. Internet starts off and asks before it goes
  on. Tools go online through Chummy, which never lets them reach this
  computer or your local network, including **Run command**'s `curl` and
  `wget`. **Run shell command** is the exception: with its Internet on, the
  programs it starts (git, pip, npm…) get an ordinary network in the
  Container sandbox, which Chummy can't check, so they can also reach this
  computer and your local network (OpenShell keeps them offline). A loop
  block can switch a tool or the internet off for itself, never on. Until tools are set up, the section is greyed
  out with a **Set up tools** link.
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
PyPI). The list shows each tool, whether it can go online, whether it's
*Potentially unsafe*, and which agents have it on; switch tools on for each
agent in **Agents**. Tools the current sandbox can't run are greyed out
with the reason.

## Channels

Talk to an agent from outside Chummy (your phone, say) while Chummy runs.
There are two kinds of channel, Matrix and Telegram; they're the same
channels chummyd has, with the same commands. Every kind is set up the same
way: **New channel**, its type, a name, the agents it serves and what that
kind asks for. When someone new wants in, the channel's card shows what to
check and asks you (the emoji for Matrix, the person for Telegram).

### Matrix

**New channel** asks for:

- **Type** (Matrix), a **name** and the **agents** people talk to (one bot
  can serve several: each person gets a room with each agent).
- **The bot's account**: the homeserver (https, or plain http only on this
  computer or your local network; **Check** says whether it answers), the
  bot's Matrix ID and its password.
  Use an account just for this: not yours, and not one a chummyd uses.
- **People** who may talk to it (Matrix IDs, one per line), and the largest
  file they may send (saved in the agent's folder, under `uploads/`).
- **Answer tool approvals in Matrix** (off by default): off, approvals wait
  for you in Chummy and the room is told; on, `/allow` and `/deny` in the
  room answer them.

Adding a channel, letting more people in or moving approvals to Matrix
asks you first. Chummy then logs in as the bot, sets up its cross-signing
and makes an encrypted room with each person and agent. Before anyone can talk to the
agent, they verify the bot from their Matrix app: the emoji show in the
channel's card, and you choose **They match** if they're the same.

The password is kept in your system keychain, and the bot's session is
sealed in the Chummy folder with a key from the keychain, so channels
connect again whenever Chummy starts. The list shows whether each one is
connected; switch one off to disconnect it without losing its settings, or
delete it (the rooms stay, but nobody can reach the agent through them).

In a room, a message starts a run of the agent's loop (shown in the
sessions panel, and in the agent's chats as a chat with that person);
replying to a question answers it; `/help` lists the commands (`/stop`,
`/pause`, `/resume`, `/new`, `/status`, `/jobs`, `/run`…). The connection
to the homeserver is Chummy's own, like model connections, so the Internet
permission doesn't apply to it.

### Telegram

Telegram isn't end-to-end encrypted: Telegram's servers can read what's
said to the bot and what the agent answers. Choose Matrix for anything
private.

1. **Make a bot** in Telegram: talk to @BotFather, send `/newbot`, and copy
   the token it gives (it looks like `123456:ABC-def…`). In its **Bot
   Settings**, turning **Allow Groups** off is best (the bot leaves groups
   anyway). Give each channel its own bot, and don't use one a chummyd uses.
2. **New channel**: type **Telegram**, a name, the agent, and the token.
   **Show answers as they're written** (on by default) shows the answer
   growing as the agent writes it; off, it comes when it's done. **Bot API
   server** stays Telegram's own unless you run one (**Check** says
   whether it answers like one). Files people send can
   be up to 20 MB (Telegram's limit for bots). **Answer tool approvals in
   Telegram** (off by default) puts Allow and Deny buttons in the chat.
3. **Let people in.** Once the channel is connected, **Add a person**
   makes a six-digit code that works once, for ten minutes. The person
   sends `/start <code>` to the bot; their name and Telegram id show in the
   channel's card (with a notification), and you choose **Let them in** or
   **Refuse**. Messages from anyone else get no answer at all.

The people let in are listed in the card, each with **Remove**. The token
is kept in your system keychain and the people let in are sealed in the
Chummy folder, so the channel connects again whenever Chummy starts. A
different bot token starts with nobody let in.

In the chat, a message starts a run; replying to a question answers it;
the menu next to the message box lists the commands (`/new`, `/status`,
`/stop`…).

## Security and Privacy

- **Internet access** and **Tools**: the app-wide switches (off by
  default).
- **Ask before tools go online** (on by default).
- **Sandbox**: where tool calls run. **Native** (built in, WebAssembly, the
  default), **Container** (Docker or Podman, using the image you approve:
  one locked-down container for each agent's folder, kept while Chummy runs;
  the folder is the only place it can write, and programs' settings and
  user installs go in `.chummy-home` there, so they stay between calls) or
  **NVIDIA OpenShell**. See
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
