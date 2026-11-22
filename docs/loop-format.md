# The Chummy loop format (`chummy.loop`, version 1)

A loop is how an agent thinks: a directed graph of **blocks** that pass text
to each other along **links**, from the user's message to the agent's reply.
Loops are saved as JSON files. Every object rejects unknown fields, so a valid
file means exactly one thing.

## File

```json
{
  "format": "chummy.loop",
  "version": 1,
  "name": "Simple chat",
  "description": "Answers each message with the recent conversation as context.",
  "limits": { "max_steps": 200, "max_llm_calls": 20 },
  "blocks": [ … ],
  "links": [ … ]
}
```

| Field         | Type   | Required | Meaning |
|---------------|--------|----------|---------|
| `format`      | string | yes      | Always `"chummy.loop"`. |
| `version`     | number | yes      | Format version. This document describes `1`. Newer versions are refused, never guessed at. |
| `name`        | string | yes      | Display name. |
| `description` | string | no       | Defaults to `""`. |
| `limits`      | object | no       | Safety limits for one run (see below). |
| `blocks`      | array  | yes      | The blocks. |
| `links`       | array  | no       | The links. Defaults to `[]`. |

### Limits

| Field           | Default | Range     | Meaning |
|-----------------|---------|-----------|---------|
| `max_steps`     | 200     | 1–10,000  | Most block runs in one run of the loop. |
| `max_llm_calls` | 20      | 1–1,000   | Most model calls in one run of the loop. |

A run that reaches either limit stops and reports it, so a cycle can't run forever.

### Block

```json
{ "id": "answer", "type": "llm", "label": "Answer",
  "position": { "x": 600, "y": 60 },
  "params": { "prompt": "_INPUT_\n\nWrite the assistant's reply.", "max_tokens": 1024 } }
```

| Field      | Type   | Required | Meaning |
|------------|--------|----------|---------|
| `id`       | string | yes      | Unique in the loop. 1–64 characters: letters, digits, `-`, `_`. |
| `type`     | string | yes      | A block type from the table below. |
| `label`    | string | no       | Display name. |
| `position` | object | no       | `{ "x": number, "y": number }` on the editor canvas. Doesn't affect running. |
| `params`   | object | no       | Type-specific parameters. Unknown keys are errors; missing keys take their defaults. |

### Link

```json
{ "from": { "block": "answer", "port": "out" }, "to": { "block": "reply", "port": "in" } }
```

A link carries **every** value an output port produces to one input port. The
same link can't appear twice. An output can feed any number of inputs, and an
input can receive from any number of outputs.

## How a loop runs

- **Values** are either *text* or a *list* of texts. Each port declares which it
  carries: `text`, `list`, or `any`. A link must connect matching kinds (`any`
  matches both).
- Every input port has a **queue**. A value arriving on any link into the port
  joins the back of its queue.
- A block is **ready** when every one of its input ports has at least one
  value. It then takes one value from the front of each queue and runs.
  Multi-input blocks therefore wait for all their inputs.
- A block's results go to **every** link from the output port that produced
  them (broadcast). Most blocks produce one value on each output. *For each*
  produces one value per list item, *Branch* uses only one of its two outputs,
  and *Reply* produces none.
- Blocks with no inputs (*Message*, *Conversation*, *Fixed text*) run once, at
  the start, in file order.
- Ready blocks run **one at a time, first come, first served**, so a run is
  deterministic. The run ends when nothing is ready.
- **Cycles are allowed.** A block runs again whenever its input queues refill.
  Use a *Branch* to leave the cycle, and rely on `limits` as a backstop.
- The agent's reply is everything the *Reply* blocks produced, in order.
- At the end, blocks still holding some but not all of their inputs are
  reported as *waiting*, which usually means a link is missing.

## Block types

Ports are written `name: kind`.

| Type | Name in the editor | Inputs | Outputs | Parameters (default) |
|------|-------------------|--------|---------|----------------------|
| `input` | Message | none | `out: text` | none. Sends the chat message. |
| `history` | Conversation | none | `out: text` | `max_messages` (10): recent messages, formatted `User: …` / `Assistant: …`. |
| `fixed` | Fixed text | none | `out: text` | `text` (""). |
| `llm` | Ask the model | `in: text` | `out: text` | `prompt` ("_INPUT_"), `system` (""), `model` (unset = the agent's model; otherwise a model connection id), `temperature` (unset = model default; 0–2), `max_tokens` (1024). `_INPUT_` in the prompt is replaced by the input; if it's missing, the input is added after the prompt. `<think>…</think>` sections are removed from the answer. |
| `combine` | Combine | `in1` … `inN: text` | `out: text` | `inputs` (2; 1–10), `format` ("_INPUT1_\n\n_INPUT2_"): `_INPUTi_` is replaced by input *i*. |
| `split` | Split into chunks | `in: text` | `out: list` | `chunk_size` (2000), `overlap` (0), `deviation` (50): cuts at a newline, period, comma or space within `deviation` characters of the ideal cut. |
| `join` | Join list | `in: list` | `out: text` | `separator` ("\n"). |
| `regex` | Find matches | `in: text` | `out: list` | `pattern`: a regular expression; outputs every match. |
| `capture` | Capture groups | `in: text` | `group1` … `groupK: list` | `pattern`: one output per capture group, each a list of that group's text in every match. |
| `branch` | Branch | `value: any`, `test: text` | `yes: any`, `no: any` | `pattern` ("(?i)^\\s*yes"): if `test` matches, `value` goes out of `yes`, otherwise out of `no`. |
| `forward` | Pass through | `in: any` | `out: any` | none. |
| `remember` | Remember first | `in: any` | `out: any` | none. Keeps the first value and sends it on every later activation. |
| `for_each` | For each | `in: list` | `out: text` | none. Sends each item separately. |
| `separate` | Pick items | `in: list` | `out1` … `outN: text` | `outputs` (2; 1–10): item *i* goes out of `outi`. |
| `read_file` | Read file | `path: text` | `out: text` | none. Reads a file in the agent's workspace (up to 1 MB). |
| `write_file` | Write file | `path: text`, `content: text` | `out: text` (the path) | `append` (false). Creates folders as needed. |
| `list_files` | List files | `path: text` | `out: list` | none. Folder names end with `/`. Use `.` for the workspace itself. |
| `output` | Reply | `in: text` | none | none. Adds its input to the agent's reply. |

Regular expressions use Rust's [`regex`](https://docs.rs/regex) syntax. Start
a pattern with `(?i)` to ignore case.

File paths in workspace blocks are always relative to the agent's workspace.
Absolute paths, `..`, and links that point outside the workspace are refused.

## Validation

Chummy checks a loop when it's opened, edited, imported or run.

**Errors** (the loop can't run): wrong `format` or `version`; bad or duplicate
block ids; unknown block types; bad parameters or invalid regular expressions;
links to missing blocks or ports; links between mismatched kinds; duplicate
links; limits out of range; no starting block.

**Warnings** (the loop runs, probably not as intended): no *Reply* block; an
input port with nothing connected, so its block never runs; a block that can't
be reached from any starting block.

## Example: draft and self-check

This template ships with Chummy. It drafts a reply, asks the model to check
it, and improves it until the check says YES (or the 12-call limit is reached).

```text
Conversation ─┐
              ├─ Combine ─ Draft ─┬──────────────── Branch.value ── yes ─ Reply
Message ──────┘                  └─ Check ─ Branch.test
                                    ▲          └── no ─ Improve ─┬─ Branch.value
                                    └────────────────────────────┘
```

Export it from the loop editor to see the full file.
