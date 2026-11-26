# chummyd: agents that run on their own

chummyd runs your agents unattended, on this computer or another one (a home
server, a small VPS, a container): their scheduled jobs, and conversations with
them. You set it up in the Chummy app; chummyd then runs exactly that, with the
same code the app uses, until it stops.

- **Configured in the app, then fixed.** Nothing can change chummyd's
  configuration while it runs. To change it, export a new bundle and start
  chummyd again.
- **No way in from the network.** chummyd listens on nothing. You talk to its
  agents through *channels*: for now the terminal it runs in.
- **Agents stay in their folders.** Each agent works only in
  `<jail dir>/<agent>/`, which never contains (and is never inside)
  chummyd's own files.

## 1. Make a bundle

Open **Configure chummyd** from the main menu.

1. Name it, check the time zone (job schedules use it), and choose how many
   jobs may run at once.
2. Include the agents it runs. Each brings its loop, model connection, tools
   and settings. An agent that uses Chummy's built-in engine can't be included:
   chummyd has no built-in engine, so give it another model (Ollama, say).
3. **Answer its approvals over channels** is off for each agent. Off, tool
   approvals and daily-limit questions are refused when asked over a channel,
   so tools that ask first won't run. Turn it on only for an agent that needs
   it: a remote "allow" is what a prompt injection would hope for.
4. Include the jobs it runs (their agent must be included).
5. Choose the channels, and whether chummyd's logs may include message text
   (off: they say what happened, never what was said).
6. **Check** shows what chummyd would say at start-up: errors to fix, and
   warnings worth knowing.
7. Choose a password, or **Generate** one. The bundle holds the API keys of
   the model connections it uses: anyone with both the bundle and the password
   can read them. Keep both safe, and apart.
8. **Export bundle…** warns once more, asks where to save it (a `.chummyd`
   file) and seals it.

**Changing a bundle chummyd already uses:** **Open a bundle…** first (with its
password), change what you need, and export again. It stays the same chummyd,
so its conversations and paused runs carry on. A bundle made from scratch is a
new chummyd, and an old state folder won't open with it (see below).

## 2. Run it

```sh
chummyd run --bundle home.chummyd --jail-dir /srv/chummy-jails
```

- chummyd asks for the password (it isn't shown). Without a terminal, give it
  `--password-file <file>` (one line; make it private: `chmod 600`). There's no
  way to put the password on the command line, where other programs could see
  it.
- `--jail-dir` is where the agents' folders go: `<jail dir>/<agent id>/`, made
  if they're missing. It can't hold the bundle, the password file or the state
  folder, or be inside them.
- `--state <folder>` sets the state folder (default: beside the bundle, named
  `<bundle>.state`).
- chummyd refuses to run as root or Administrator.
- `chummyd check --bundle <file>` opens a bundle and says what's in it.

### The state folder

chummyd keeps conversations, paused runs, job history and the day's model use
in its state folder, sealed with a key that's in the bundle. It's saved when
something changes (at most every 30 seconds), after each run, and when chummyd
stops. Only one chummyd can use a state folder at a time.

If the state folder can't be opened (it's from another bundle, or it was
changed), chummyd won't start: it never starts over by itself. Move the folder
aside to start afresh. Restoring an old copy of the folder brings back what was
in it then.

### Stopping

Ctrl-C, `SIGTERM` (as `docker stop` sends) or `/quit` in the terminal. Each
conversation's run pauses (cutting a model call short if it must; it's asked
again later), then chummyd saves and stops. Paused runs wait for `/resume` the
next time; they don't carry on by themselves. Runs that can't pause in time
(`--grace`, 90 seconds by default) are stopped.

### In Docker

Give the password as a Docker secret, mount the bundle read-only, and keep the
jail dir and the state folder in volumes:

```yaml
services:
  chummyd:
    image: chummyd:latest   # or a container with the chummyd binary
    command: ["run", "--bundle", "/bundle/home.chummyd", "--jail-dir", "/jails",
              "--state", "/state", "--password-file", "/run/secrets/chummyd_password"]
    user: "10001:10001"
    read_only: true
    cap_drop: [ALL]
    security_opt: ["no-new-privileges:true"]
    stop_grace_period: 2m
    volumes:
      - ./bundle:/bundle:ro
      - jails:/jails
      - state:/state
    secrets: [chummyd_password]
    stdin_open: true   # for the terminal channel (docker attach)
    tty: true
secrets:
  chummyd_password:
    file: ./chummyd_password.txt
volumes:
  jails:
  state:
```

## 3. Talk to it (the terminal channel)

Type a message and press Enter: it goes to the agent you're talking to, and
its replies, how the run ended and its figures come back, as in the app's
chat. If the agent asked you something, your next message is the answer.

| Command | Does |
|---|---|
| `/stop` | stops the run going on |
| `/pause`, `/resume` | pauses it; carries it on where it stopped |
| `/discard` | throws away the paused run |
| `/new` | starts a fresh conversation |
| `/status` | what's running, paused or waiting |
| `/jobs`, `/run <job>` | this agent's jobs; runs one now |
| `/allow`, `/deny [reason]` | answers a tool approval (if allowed for the agent) |
| `/agents`, `/agent <name>` | lists the agents; talks to another |
| `/quit` | stops chummyd |
| `/help` | the list |

Commands are read by chummyd, never passed to the agent, and nothing an agent
says can be taken as one. Job results and questions from jobs show in the
terminal too.

The system log (what chummyd does, never message text unless the bundle
allows it) goes to standard error; the conversation to standard output.
