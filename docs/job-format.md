# Job file format (`chummy.job`, version 1)

A job runs one agent's loop, started with a message, on a schedule or when
files change in the agent's folder. Jobs live in the Chummy folder as
`jobs/<id>.json`, and can be exported and imported. The same file works in
the Chummy app and in chummyd (see [chummyd-plan.md](chummyd-plan.md)).

A job never grants anything. It names an agent, and runs with exactly that
agent's folder, model and permissions; it can't do anything the agent can't.
Imported jobs always arrive switched off.

## File

```json
{
  "format": "chummy.job",
  "version": 1,
  "id": "morning-digest",
  "name": "Morning digest",
  "description": "",
  "enabled": true,
  "agent": "agent-93285e48",
  "loop": null,
  "message": "Summarize what changed in notes/ since yesterday. Today is _NOW_.",
  "file": null,
  "triggers": [
    { "kind": "daily", "time": "08:30", "days": ["mon", "tue", "wed", "thu", "fri"] },
    { "kind": "folder", "path": "inbox" }
  ],
  "missed": "skip",
  "time_limit_minutes": 30,
  "save_reply_to": "reports/_DATE_.md",
  "notify": "problems"
}
```

| Field | Meaning |
|-------|---------|
| `format`, `version` | Always `"chummy.job"` and `1`. Newer versions are refused; unknown fields are refused. |
| `id` | Lowercase letters, digits, `-` and `_`, at most 64; it's the file name. `state` and `history` are reserved. |
| `name` | Shown to people; at most 100 characters. |
| `description` | Optional notes. |
| `enabled` | Off: the job only runs when started by hand. |
| `agent` | The id of the agent that runs it. |
| `loop` | A loop id to use instead of the agent's own (optional). |
| `message` | What the loop starts with, like a chat message. See *Placeholders*. At most 50,000 characters. |
| `file` | A file in the agent's folder to include (optional; up to 256 KB of text). |
| `triggers` | Up to 10; see below. None: runs only by hand. |
| `missed` | `"skip"` (default) or `"run_once"`: what to do about scheduled times that passed while Chummy wasn't running or the computer slept. Several missed times still run once. |
| `time_limit_minutes` | Stop a run after this long; 0 means no limit (at most 10,080). |
| `save_reply_to` | Also write the reply to this file in the agent's folder (optional). `_DATE_` and `_TIME_` may be used in the name. |
| `notify` | `"problems"` (default), `"always"` or `"never"`: when to show a notification after a run. Notifications never include the reply. |

Paths are relative to the agent's folder. Absolute paths, `~`, drive letters
and `..` are refused when the job is saved, and the folder jail checks again
whenever a path is used.

## Triggers

| Trigger | Fires |
|---------|-------|
| `{ "kind": "daily", "time": "08:30", "days": ["mon", …] }` | At that local time (24-hour) on those days; no days means every day. |
| `{ "kind": "every", "minutes": 60 }` | Repeatedly, that far apart (5 minutes to 4 weeks), counted from when the job was switched on or its schedule last changed. |
| `{ "kind": "once", "at": "2026-12-01T09:00" }` | Once, at that local date and time. |
| `{ "kind": "folder", "path": "inbox" }` | When a file directly in that folder of the agent's (`.` for the folder itself) is added or changed. Each file starts its own run. |

Times use the computer's time zone (for chummyd, the one in its config). A
time that doesn't exist on the day the clocks go forward runs just after the
jump; a time that happens twice when they go back runs the first time.

Folder triggers poll the folder through the agent's jail every 15 seconds. A
file starts a run once it has stopped changing between two polls, so
half-written files are skipped. Hidden files and temporary names (`.tmp`,
`.part`, `.crdownload`, `~$…`, `…~`) are ignored, as are files the job wrote
itself during a run. Each job starts at most 30 file runs an hour.

## Placeholders

| In `message` | Becomes |
|--------------|---------|
| `_NOW_` | The date and time, e.g. `Sunday 2026-11-22 13:28` |
| `_DATE_`, `_TIME_` | `2026-11-22`, `13:28` |
| `_FILE_` | The run's file: the one that triggered it (folder triggers), or the job's `file` |
| `_FILE_TEXT_` | That file's text |

Placeholders are filled in one pass, so text that came from a file is never
searched for placeholders itself. If there's file text and the message uses
neither `_FILE_` nor `_FILE_TEXT_`, the text is added after the message. A
triggering file that isn't readable text is passed by path only; a job's own
`file` that can't be read fails the run.

## How jobs run

- A job runs at most once at a time. A scheduled time that comes while it's
  still running, or waiting to run, is skipped and noted in its history.
- An agent does one thing at a time. A run waits while the agent is busy (a
  chat, a test run, or another job), and a chat waits while a job uses the
  agent.
- At most *N* runs go at once (Settings → General, 1 by default).
- A scheduled time noticed more than 10 minutes late counts as missed.
- **Pause scheduled jobs** stops scheduled and folder runs; times that pass
  while paused are skipped, not caught up. Runs started by hand still go.
- A job that asks the user something (an *Ask me* block, or a tool call that
  needs an OK) waits, shows the question in the agent's chat and the
  Scheduler, and sends a notification.

## State and history

Next to the job files:

```text
jobs/
  <id>.json            job definitions
  state.json           per job: checked_until, anchor, last_run; and paused
  history/<id>.json    the job's last 100 runs, newest last
```

A history record has `run_id`, `cause` (`schedule`, `catch_up`, `manual` or
`file` with its path), `status` (`completed`, `failed`, `stopped`, `limit`,
`missed` or `skipped`), `started_at`, `finished_at`, the `reply` (cut to 8,000
characters), a `note`, and how many model and tool calls the run made.
