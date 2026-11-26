# chummyd: agents that run on their own

chummyd runs your agents unattended, on this computer or another one (a home
server, a small VPS, a container): their scheduled jobs, and conversations with
them. You set it up in the Chummy app; chummyd then runs exactly that, with the
same code the app uses, until it stops.

- **Configured in the app, then fixed.** Nothing can change chummyd's
  configuration while it runs. To change it, export a new bundle and start
  chummyd again.
- **No way in from the network.** chummyd listens on nothing. You talk to its
  agents through *channels*: the terminal it runs in, and end-to-end
  encrypted Matrix rooms.
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
5. Choose the channels (for Matrix, see [below](#4-talk-to-it-in-matrix)),
   and whether chummyd's logs may include message text (off: they say what
   happened, never what was said).
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

`docker/chummyd/` has a Dockerfile and a Compose file. The image is
distroless (no shell, no package manager), holds only chummyd and what it
needs, and runs as the unprivileged user 65532. For now you build it
yourself (x86_64), from the repository's root:

```sh
docker build -f docker/chummyd/Dockerfile -t chummyd .
```

The first build takes a while: it builds the tools' Python from python.org's
source (checked against pinned hashes and signatures) and chummyd itself.
Later builds reuse what didn't change.

Then copy `docker/chummyd/compose.yaml` to a folder of your own (not inside
the repository) and put beside it:

- `bundle.chummyd`: the bundle you exported.
- `chummyd_password.txt`: its password, on one line.

**The password file has to be readable by chummyd's user in the container.**
Compose mounts it with the owner and permissions it has on your computer, so
give it to user 65532 and make it private:

```sh
sudo chown 65532:65532 chummyd_password.txt
sudo chmod 400 chummyd_password.txt
```

(Docker Swarm, Kubernetes and Podman secrets can set the owner and mode
themselves.) Then:

```sh
docker compose up -d
docker attach chummyd
```

`docker attach` is chummyd's terminal: confirm Matrix verifications there
(`/verify`), or talk to agents if the terminal channel is on. **Leave it
with Ctrl-P Ctrl-Q**: Ctrl-C stops chummyd. `docker logs chummyd` shows
what it said.

What the Compose file sets up:

- The bundle mounted read-only at `/bundle/bundle.chummyd`; the agents'
  folders in the volume `jails` (as `/jails/agents/<agent>`: chummyd won't use
  a mount point, such as a volume's root, as the jail dir) and the state folder in the volume `state`
  (`/state`). Back up `state`: it holds the conversations and Matrix keys.
- A read-only container with no capabilities and no privilege escalation.
  chummyd listens on nothing, so there are no ports. It never needs the
  Docker socket: don't mount it.
- `stop_grace_period: 2m`, longer than chummyd's 90 seconds for pausing
  runs, so `docker compose stop` (or a restart) pauses them cleanly.
- `restart: unless-stopped`. To stop chummyd for good, use
  `docker compose stop`: after `/quit` or Ctrl-C, Docker starts it again.
- A model on the same computer (Ollama, say) is at
  `http://host.docker.internal:11434` from inside the container: use that
  address in the app's model connection, and uncomment the `extra_hosts`
  line on Linux. A homeserver in the same Compose file is at its service
  name (`http://synapse:8008`, say). Both count as local, so they can use
  plain http.

**Without a password file:** run it by hand with a terminal, and without
`--password-file`:

```sh
docker run -it --name chummyd --read-only --tmpfs /tmp --cap-drop ALL \
  --security-opt no-new-privileges:true --stop-timeout 120 \
  -v "$PWD/bundle.chummyd:/bundle/bundle.chummyd:ro" \
  -v chummyd_jails:/jails -v chummyd_state:/state \
  chummyd run --bundle /bundle/bundle.chummyd --jail-dir /jails/agents --state /state
```

chummyd asks for the password (so it never touches the disk); then leave
with Ctrl-P Ctrl-Q. It can't restart by itself that way: each start waits
for the password.

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

## 4. Talk to it in Matrix

With the Matrix channel, each person you allow gets an end-to-end encrypted
room with each agent, on your own homeserver (or any homeserver). chummyd
makes the rooms and invites people; you just accept the invite.

### Set it up

1. **Make an account just for chummyd** on the homeserver, the same way you
   make any account (say `@chummyd:example.org`). chummyd logs in as it.
2. In **Configure chummyd**, turn on **Matrix** and fill in:
   - the **homeserver**'s address (https; plain http only on this computer or
     your local network). **Check the homeserver** says whether it answers.
   - **chummyd's Matrix ID** and **its password**. The password goes into the
     bundle, encrypted: chummyd uses it to log in the first time and to set up
     its cross-signing. When you open a bundle to change it, leave the password
     empty to keep the one it has.
   - **Who may talk to** each agent: Matrix IDs, separated by commas. Each
     person gets their own room with that agent.
   - Who each job **reports to**: its results and its questions go to that
     person's room with the job's agent.
   - The **largest file** people can send (100 MB unless you change it).
3. Export the bundle and start chummyd **with a terminal** (or `docker
   attach` to it): you'll confirm verifications there.

On its first start chummyd logs in, sets up cross-signing, and creates one
room per agent and person, named like "Helper (My chummyd)". It keeps its
session and its encryption keys in the state folder (the keys in
`<state>/matrix/`, encrypted), so later starts carry on as the same session.
Key backup is off: the keys never leave the state folder.

### Verify chummyd

chummyd only acts on messages from **verified sessions**, so the first thing
each person does is verify it:

1. Accept the invite to the room. Until you verify, chummyd answers once
   with "Verify this session to talk to chummyd." and ignores your messages.
2. In your Matrix app, open chummyd's profile in the room and choose
   **Verify** (then **Verify by emoji**).
3. chummyd shows the same seven emoji in its terminal:

   ```text
   chummyd: @you:example.org (session ABCDEFGHIJ) wants to verify chummyd. Check that they see these emoji, in this order:
              🐶 Dog   🔑 Key   🌙 Moon   …
            Then: /verify 1 yes if they match, /verify 1 no if they don't.
   ```

   Compare them with what your app shows. If they match, type
   `/verify 1 yes` in chummyd's terminal (or `/verify yes` when only one is
   waiting) and confirm in your app too. If they don't, `/verify 1 no`:
   something is in between, and the verification is cancelled.

`/verify` works in chummyd's terminal even when the terminal isn't a channel
in the bundle. Without a terminal at all, verifications can't be confirmed
(chummyd says so in its log).

### Talk

Send a message in the room: it goes to that agent. Its reply comes back as
one message that grows as the run goes (edited at most every two seconds),
with how the run ended and its figures at the end; long replies are split
over several messages. While it works, chummyd shows as typing (not while
it waits for your answer to a question). The commands are the terminal's, except `/agents`,
`/agent` and `/quit` (each room talks to one agent, and only the operator
stops chummyd).

- **Questions** from the agent come as their own message. Answer by
  **replying** to it, or just send your answer when it's the only question
  waiting.
- **Approvals** (if allowed for the agent) come the same way: answer with
  `/allow` or `/deny [reason]`.
- **Files** you send are saved in the agent's folder under `uploads/` (with a
  safe name, never replacing a file) and chummyd says where. Sending a file
  doesn't start a run: tell the agent about it in a message.
- **Jobs** that report to you post their results and questions in your room
  with their agent.

### What chummyd ignores

chummyd acts only on a message that's in one of its rooms, from that room's
own person, encrypted, and sent from a session that's verified. Everything else
is ignored: other people, edits, reactions, deletions, and invites to other
rooms (refused). If someone else joins a room, chummyd ignores it until they
leave. A message sent while chummyd was off for more than ten minutes isn't
run: chummyd says so, and you can send it again.

If the homeserver can't be reached, chummyd keeps running (jobs too) and
tries again, waiting longer each time. If you remove a person or an agent
from the bundle, chummyd leaves their room the next time it starts; someone
who left their room is invited back.
