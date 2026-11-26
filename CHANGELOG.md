# Changelog

What changed in each release, newest first. The release workflow puts the
section for the version it releases at the top of the GitHub release's
notes, and stops before building anything if there isn't one.

For the next release, write under **Unreleased** as you go, then rename it to
the version (`## v0.1.5`) in the commit you tag. Keep each line short and
about what people notice; `scripts/release-notes.mjs v0.1.5` shows what the
release will say.

## Unreleased

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
