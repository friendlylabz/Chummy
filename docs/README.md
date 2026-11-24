# Chummy documentation

- **[user/](user/README.md)**: for people using Chummy. Installing it,
  setting up a first agent, the loop editor and how to build loops that
  work well with small models, and every setting.
- **[dev/](dev/README.md)**: for people working on Chummy's code. What each
  module does, how to build and test, and how the security features fit
  together.

Reference documents (file formats and design notes, read by both):

| File | What's in it |
|---|---|
| [loop-format.md](loop-format.md) | The `chummy.loop` format, how a loop runs, and every block type. |
| [tool-format.md](tool-format.md) | The tool file format and the built-in tools. |
| [job-format.md](job-format.md) | Scheduled jobs (`chummy.job`). |
| [sandboxes.md](sandboxes.md) | Where tools run: Native, Container and OpenShell. |
| [security.md](security.md) | The threat model, the rules, and audit notes. |
| [chummyd-plan.md](chummyd-plan.md) | The plan for chummyd, the optional background service. |
