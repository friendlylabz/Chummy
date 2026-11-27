# Changelog

What changed in each release, newest first. The release workflow puts the
section for the version it releases at the top of the GitHub release's
notes, and stops before building anything if there isn't one.

For the next release, write under **Unreleased** as you go, then rename it to
the version (`## v0.1.5`) in the commit you tag. Keep each line short and
about what people notice; `scripts/release-notes.mjs v0.1.5` shows what the
release will say.

## Unreleased

- **Breaking:** tools are switched on per agent. Each agent has a tool
  switchboard (Settings → Agents): for every tool, **On**, **Ask first** and
  **Internet**. The per-tool switches in Settings → Tools are gone (it now
  shows which agents use each tool), and a loop block can still switch tools
  or the internet off for itself. Settings from older versions aren't moved
  over: set your agents' tools again. chummyd bundles are a new version too:
  export yours again.
- A new agent can be set up completely before you create it: tools, model
  settings and Agent Loop Configuration. The tools section is greyed out with
  a **Set up tools** link until tools are set up, and what you've filled in is
  kept if you go to another pane to add a model or set up tools.
- The **Chummy assistant** comes in three sizes. **Small** (for models up to
  about 14B) steers more: it checks the last change, asks you when it's
  unclear, checks its answer before it's final and keeps its context light.
  **Medium** is the one before, and still the default. **Large** (70B and
  frontier models) isn't steered: it plans and does the job in one go and
  uses its context to remember. The Terminal assistant is gone; unedited
  copies become the medium assistant.
- Models on the built-in engine have an edit button: name, longest answer,
  reasoning effort, top P and K, min P, repeat penalty, seed, stop sequences,
  context and daily limit. The engine now uses all of these.
- The longest answer a model can be asked for is now 262,144 tokens (was
  32,768).
- chummyd: how many messages each chat keeps (was 500, now 2,000 by default)
  and how many chats each agent keeps (50) are set in the bundle (Configure
  chummyd).
- `chummyd-docker-init` no longer asks about your own certificates, and the
  Compose file no longer has that line. Mount your CA bundle yourself if you
  need it.
- Settings → Tools: the internet and "ask first" switches no longer sit out
  of line (they moved to each agent).
- **Channels in the app** (Settings → Channels): talk to an agent from
  Matrix while Chummy runs, with the same channel and commands as chummyd
  (now shared code, so both behave the same). Each channel has a type, the
  agents it serves, a bot account and the people allowed. People
  verify the bot by emoji, which you confirm in the channel's card. The
  password stays in the keychain and the login is sealed, so channels
  connect again by themselves. Their runs show in the sessions panel.
- **Telegram channels**, in the app (Settings → Channels) and in chummyd
  (Configure chummyd): a bot of its own (from @BotFather) for each agent.
  People ask to talk to it with a code you make (**Add a person**, or
  `/invite` in chummyd's terminal), and you let them in (in the channel's
  card, or with `/verify` in chummyd's terminal); anyone else gets no
  answer. Answers show as they're written, approvals have
  Allow and Deny buttons, and files up to 20 MB are saved to the agent's
  `uploads/`. Private chats only: the bot leaves groups. Telegram isn't
  end-to-end encrypted, and the app says so where you set it up.
- **Breaking:** channels are now one kind of thing with a type, in the app
  and in chummyd, so new types (Signal, say) can be added without touching
  the rest. In chummyd, a Matrix channel lists its agents and its people
  (one per line), and each channel (and the terminal) has its own **Answer
  approvals in it** switch, instead of one per agent. chummyd's terminal
  answers whatever a channel asks with `/verify n yes|no` (Matrix emoji,
  someone asking in on Telegram), makes Telegram codes with `/invite`, and
  `/remove <id>` replaces `/unpair`. Bundles and chummyd's state folder are
  new versions: export the bundle again and start chummyd with a new state
  folder. Channels set up in the app before this are dropped: add them
  again (their bots need verifying again).
- **Check** next to a channel's server address (homeserver, Bot API
  server) says whether it answers like one.
- Sessions panel: each run has a link to its full log, the pane button has
  a thought-bubble icon, and **Recent runs** keeps each agent's three
  latest runs with their pane and log. Paused runs are under **Paused
  runs** (resume or discard them there), not among the recent ones, so a
  resumed run shows once, as running.

## v0.1.5

<!-- What's changed since the last release. -->

- **Breaking:** chummyd bundles are a new version. Export yours again from
  Chummy; older ones aren't opened. chummyd's password file now holds the
  password's hash, never the password: make it with `chummyd hash-password`
  (or `chummyd-docker-init`). It keeps working for every bundle you export
  later for the same chummyd with the same password.
- chummyd in Docker: **`chummyd-docker-init`** sets it up in one go. It asks
  for the bundle, its password and where the agents' folders go (a Docker
  volume or a folder on your computer), gives the files to chummyd's user,
  shows what the bundle sets up before anything runs, and writes the Compose
  file.
- `chummyd check` describes what a bundle sets up: where its models are, what
  each agent may do, its jobs and who can talk to it, and what's worth a
  second look.
- chummyd says who it runs as when it starts, and warns about files in an
  agent's folder the agent can't change (copied in as root, say).
- Chummy names exported bundles `bundle.chummyd`, the name chummyd's Docker
  setup expects.
- Release notes come from this changelog.

## v0.1.4

- **chummyd**, Chummy's agents running on their own (on a server, say): the
  same agents, loops, tools and jobs, from an encrypted bundle exported in
  **Configure chummyd**, with no app and no window. See
  [chummyd](docs/user/chummyd.md).
- chummyd talks to you in its terminal, or in **end-to-end encrypted Matrix
  rooms**: one per agent and person, verified by emoji, with replies that grow
  as the agent works, questions you answer by replying, files you send saved
  to the agent's `uploads/`, and a typing indicator while it works.
- chummyd in **Docker**: a small, locked-down image to build from
  `docker/chummyd/`, with a Compose example.
- Jobs run beside an agent's chats, like background chats, instead of
  locking them, and an agent's jobs can run side by side.
- A paused run stays one response when you resume it, with its own Resume
  and Discard buttons.
- The main menu is shorter, and the Scheduler is now the **Automation** menu.
