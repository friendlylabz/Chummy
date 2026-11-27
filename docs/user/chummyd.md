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
   jobs may run at once, how many messages each chat keeps (2,000 by
   default; older ones are dropped) and how many chats each agent keeps
   (50).
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
chummyd run --bundle home.chummyd --jail-dir /srv/chummy-jails --state /srv/chummyd-state
```

- chummyd asks for the password (it isn't shown). Without a terminal, give it
  `--password-file <file>`: a file holding the password's **hash**, never the
  password itself (a file with the password in it is refused). Make it with

  ```sh
  chummyd hash-password --bundle home.chummyd > chummyd_password.txt
  chmod 400 chummyd_password.txt
  ```

  It asks for the password, checks it opens the bundle, and writes one line.
  The hash opens this chummyd's bundles (keep the file private), but doesn't
  give away the password, which you may use elsewhere. Every bundle you
  export later for the same chummyd with the same password opens with it
  too. There's no way to put the password on the command line, where other
  programs could see it.
- `--jail-dir` is where the agents' folders go: `<jail dir>/<agent id>/`, made
  if they're missing. It can't hold the bundle, the password file or the state
  folder, or be inside them.
- `--state <folder>` is the state folder (below), made if it's missing. It's
  always needed: chummyd never guesses where to keep what it mustn't lose.
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
yourself (x86_64), from the repository's root.

**First, the tools' Python.** The image takes it ready-made from
`wasi-python/python-wasi.zip`, so the build itself never downloads it.
Take it from a Chummy release (the same one the app has inside):

```sh
mkdir -p wasi-python && cd wasi-python
curl -fLO https://github.com/friendlylabz/chummy/releases/latest/download/python-wasi.zip
curl -fLO https://github.com/friendlylabz/chummy/releases/latest/download/python-wasi.zip.sha256
sha256sum -c python-wasi.zip.sha256
cd ..
```

The release's workflow built it from python.org's signed source and signed
where it came from. To check that too (with the GitHub CLI):
`gh attestation verify wasi-python/python-wasi.zip --repo friendlylabz/chummy`.
Or build it yourself with `scripts/build-wasi-python.sh` (about 5 minutes;
it puts it in `wasi-python/`). If you build from an older or newer clone
than the latest release, take the zip from that version's release instead
(`…/releases/download/v0.1.3/…`).

**Then the image:**

```sh
docker build -f docker/chummyd/Dockerfile -t chummyd .
```

It checks the zip against `python-wasi.zip.sha256` when that's there, and
stops if the zip is missing. The first build takes a while (chummyd is
built fully optimized); later builds reuse what didn't change. It downloads
only the base images, Rust and its crates.

**Then set it up with `chummyd-docker-init`**, from anywhere, in a terminal:

```sh
~/chummy/docker/chummyd/chummyd-docker-init ~/chummyd
```

It asks:

1. **The folder** for chummyd's files (the argument, or it asks; not inside
   the repository).
2. **The bundle** you exported. It copies it into that folder as
   `bundle.chummyd` and leaves yours as it was.
3. **Where the agents' folders go:** a Docker volume (Docker keeps it), or a
   folder on this computer (default: `jails/` in the same folder), where you
   can see and back up their files. In a folder, each agent works in
   `<folder>/agents/<agent>`. A folder that already has files can't hold
   chummyd's own files, and anything in it not owned by chummyd's user can
   be given to it (it asks).
4. **The bundle's password**, never shown, never in your shell's history.

Then it readies everything for chummyd's user in the container (65532), with
sudo, since giving files to another user takes root:

- `bundle.chummyd`: owned by 65532, readable only by it (mode 400);
- `chummyd_password.txt`: the password's **hash**, worked out by the image,
  which also checks the password opens the bundle. The password itself is
  never written anywhere. Owned by 65532, mode 400;
- the agents' folder (if on this computer): owned by 65532, only it can open
  it (mode 700; look inside with sudo).

It checks those owners and modes, then **opens the bundle and says what it
sets up**, without running anything (in a container with no network, as
chummyd's user): where its models are, what tools and internet access each
agent has, its jobs, who can talk to it, and a short "Look at" list of
anything worth a second look (internet access, approvals over channels,
plain http to somewhere that isn't local). If that's not what you expected,
answer no and nothing is set up to run. Otherwise it writes `compose.yaml`
there, set up with your choices. It stops, saying why, if something's wrong:
a wrong password, no image (`CHUMMYD_IMAGE` sets its name if it isn't
`chummyd`), and so on. Run it again after exporting a new bundle.

`chummyd check --bundle <file> --password-file <file>` shows the same
description anywhere.

(By hand instead: copy `docker/chummyd/compose.yaml`, which uses volumes,
and put the bundle and `chummyd_password.txt` (made with `chummyd
hash-password`, above) beside it, both owned by 65532:65532 with mode 400. Docker Swarm, Kubernetes and Podman secrets can set
the owner and mode themselves.)

Then, in that folder:

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
  folders in the volume `jails`, or your folder, at `/jails` (each agent in
  `/jails/agents/<agent>`: chummyd won't use a mount point, such as a volume's
  root, as the jail dir); the state folder in the volume `state` (`/state`).
  Back up `state`: it holds the conversations and Matrix keys.
- Everything runs as user 65532: chummyd, and the agents' tools, which run
  inside it (WebAssembly). So everything in the agents' folders should
  belong to 65532. chummyd makes their folders itself; on each start it says
  in its log who it runs as, and warns about anything in an agent's folder
  it can't change. **Files you copy in** with `docker cp` (or as root on the
  host) belong to root, and the agent can read them but not change them.
  Give them to chummyd's user:

  ```sh
  docker run --rm -v chummyd_jails:/jails debian:trixie-slim chown -R 65532:65532 /jails
  # agents' folders on this computer instead:
  sudo chown -R 65532:65532 ~/chummyd/jails
  ```

  (`chummyd_jails` is the volume's name when your Compose folder is called
  `chummyd`: it's the folder's name, then `_jails`. Files people send in
  Matrix are saved as chummyd's own.)
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

(The Chummy app has the same Matrix channel, in Settings → Channels, for
talking to agents while the app runs. Give chummyd and the app different
bot accounts. For Telegram, see section 5.)

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

## 5. Talk to it in Telegram

Each agent can have a Telegram bot of its own. Telegram isn't end-to-end
encrypted: Telegram's servers can read what's said to the bot and what the
agent answers. Choose Matrix for anything private.

### Set it up

1. **Make a bot** for the agent: talk to @BotFather in Telegram, send
   `/newbot`, and copy the token. In its **Bot Settings**, turn **Allow
   Groups** off (chummyd's bot leaves groups anyway). Don't use a bot the
   Chummy app (or another chummyd) uses: only one program can read a bot's
   messages.
2. In **Configure chummyd**, under Channels → Telegram, switch on the
   agent's bot and paste the token. **Show answers as they're written** is
   on by default; files people send can be up to 20 MB.
3. Export the bundle and run chummyd **with a terminal** (with Docker:
   `docker compose up -d`, then `docker attach` to it): that's where you let
   people in. `chummyd check` lists the bots by their id, never their token.

### Let someone in

In chummyd's terminal, talking to the agent (`/agent <name>` switches):

- `/pair` makes a six-digit code for that agent's bot. It works once, for
  ten minutes.
- The person sends `/start <code>` to the bot. chummyd shows their name and
  Telegram id, and asks: `/pair 1 yes` lets them in, `/pair 1 no` doesn't.
  Only say yes to the person you gave the code to.
- `/people` lists who may talk to the bot; `/unpair <id>` stops someone.

The people let in are kept in the state folder (sealed), so they stay let
in when chummyd starts again. Anyone else who writes to the bot gets no
answer at all.

### Talk

As in Matrix: a message starts a run, shown as it's written; reply to a
question to answer it; approvals (when the bundle allows them over channels
for the agent) come with **Allow** and **Deny** buttons; files go to the
agent's `uploads/`. The bot's menu lists the commands. Jobs don't report to
Telegram yet.
