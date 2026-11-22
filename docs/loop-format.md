# The Chummy loop format (`chummy.loop`, version 1)

A loop is how an agent thinks: a directed graph of **blocks** that pass text
(and conversations with the model) to each other along **links**, from the
user's message to the agent's reply.
Loops are saved as JSON files. Every object rejects unknown fields, so a valid
file means exactly one thing.

## File

```json
{
  "format": "chummy.loop",
  "version": 1,
  "name": "Simple chat",
  "description": "Answers each message with the recent conversation as context.",
  "limits": { "max_steps": 200, "max_llm_calls": 20, "max_tool_calls": 20 },
  "blocks": [ … ],
  "links": [ … ],
  "functions": [ … ]
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
| `functions`   | array  | no       | Block functions used in this loop (see below). Omitted when empty. |

### Limits

| Field           | Default | Range     | Meaning |
|-----------------|---------|-----------|---------|
| `max_steps`     | 200     | 1–10,000  | Most block runs in one run of the loop. |
| `max_llm_calls` | 20      | 1–1,000   | Most model calls in one run of the loop, retries included. |
| `max_tool_calls`| 20      | 1–1,000   | Most tool calls in one run of the loop. |

Blocks inside functions count toward the same limits.

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

- **Values** are *text*, a *list* of texts, or an *exchange* (a conversation
  with the model; see below). Each port declares which it carries: `text`,
  `list`, `exchange`, or `any`. A link must connect matching kinds (`any`
  matches all). Where a block reads an exchange as text, it uses its
  transcript (`User: …\n\nAssistant: …`).
- Every input port has a **queue**. A value arriving on any link into the port
  joins the back of its queue.
- A block is **ready** when every one of its input ports has at least one
  value. It then takes one value from the front of each queue and runs.
  Multi-input blocks therefore wait for all their inputs.
- A block's results go to **every** link from the output port that produced
  them (broadcast). Most blocks produce one value on each output. *For each*
  produces one value per list item, *Branch* uses only one of its two outputs,
  a model block sends either *Answer* or *Tool call* (plus *Reasoning*), and
  *Reply* produces none.
- Blocks with no inputs (*Message*, *Conversation*, *Fixed text*) run once, at
  the start, in file order.
- Ready blocks run **one at a time, first come, first served**, so a run is
  deterministic. The run ends when nothing is ready.
- **Cycles are allowed.** A block runs again whenever its input queues refill.
  Use a *Branch* to leave the cycle, and rely on `limits` as a backstop.
- The agent's reply is everything the *Reply* blocks produced, in order.
- At the end, blocks still holding some but not all of their inputs are
  reported as *waiting*, which usually means a link is missing.

## Exchanges and tools

An **exchange** is a list of messages, each with a role (`system`, `user`,
`assistant`, `tool`). Assistant messages may carry tool calls; tool messages
answer one call. Exchanges let a loop hold a running conversation with the
model and let the model use tools.

A model block (`llm`) with tools works like this:

1. Its input is text (filled into its prompt, as a new conversation) or an
   exchange (which it continues; the prompt isn't used).
2. The model's tools are the block's `tools` that are installed and switched
   on; any others are left out, with a notice.
3. If the model answers, the answer goes out of **Answer**. If it calls a
   tool, the exchange plus the model's request goes out of **Tool call**
   instead, and Answer stays silent. **Reasoning** carries any thinking the
   model showed (`<think>` sections or the provider's reasoning) in both
   cases, possibly empty. Unconnected outputs are harmless.
4. Calls are checked first: the tool must be one of the block's, and the
   arguments must match its parameters. Numbers and true/false written as
   text are fixed quietly; anything else is a bad call, as is not calling a
   tool when `tool_choice` is `required`. `on_bad_call` decides what happens:
   `retry` (tell the model what was wrong and ask again, up to `retries`
   times, then stop), `fail` (stop), or `answer` (use the text as the answer).

A **Run tool** block runs the calls at the end of an exchange in the agent's
sandbox, adds their results as tool messages, and sends the exchange on.
Linking it back into the model block lets the model continue with the
results:

```text
           ┌──────── Run tool ◀─── Tool call ┐
text ─▶ Ask the model ─────────────────────── ┘
                    └─ Answer ─▶ Reply
```

How tools themselves are written, installed and sandboxed is described in
[tool-format.md](tool-format.md).

## Block types

Ports are written `name: kind`.

| Type | Name in the editor | Inputs | Outputs | Parameters (default) |
|------|-------------------|--------|---------|----------------------|
| `input` | Message | none | `out: text` | none. Sends the chat message. |
| `history` | Conversation | none | `out: text` | `max_messages` (10): recent messages, formatted `User: …` / `Assistant: …`. |
| `fixed` | Fixed text | none | `out: text` | `text` (""). |
| `llm` | Ask the model | `in: any` | `out: text` (Answer), `tool: exchange` (Tool call, only with tools), `reasoning: text` | `prompt` ("_INPUT_"), `system` (""), `model` (unset = the agent's model; otherwise a model connection id), `temperature` (unset = model default; 0–2), `max_tokens` (1024), `tools` ([]), `tool_choice` ("auto" or "required"), `on_bad_call` ("retry", "fail" or "answer"), `retries` (2; 0–5). Text input: `_INPUT_` in the prompt is replaced by it (or it's added after the prompt). Exchange input: the exchange is continued; a non-empty `system` replaces its system message for this call. `<think>…</think>` sections are moved from the answer to Reasoning. |
| `run_tool` | Run tool | `in: exchange` | `out: exchange` | `max_result_chars` (4000), `on_error` ("tell_model" or "fail"). Runs the pending tool calls at the end of the exchange and adds their results. Longer results are cut. |
| `exchange` | Start exchange | `in: any` | `out: exchange` | `system` (""), `roles` (["user"]). Text becomes one message with the first role; a list becomes one message per item, with the roles in turn. |
| `augment` | Add to exchange | `exchange: exchange`, `in: any` | `out: exchange` | `role` ("user"), `format` ("_INPUT_"). Adds the text as a message (`_INPUT_` replaced by it); a list adds one message per item. |
| `read_exchange` | Read exchange | `in: exchange` | `last: text`, `all: text` | `role` ("assistant"; or "user", "tool", "system", "any"). `last` is the last message from that role, `all` the transcript. |
| `combine` | Combine | `in1` … `inN: text` | `out: text` | `inputs` (2; 1–10), `format` ("_INPUT1_\n\n_INPUT2_"): `_INPUTi_` is replaced by input *i*. |
| `split` | Split into chunks | `in: text` | `out: list` | `chunk_size` (2000), `overlap` (0), `deviation` (50): cuts at a newline, period, comma or space within `deviation` characters of the ideal cut. |
| `join` | Join list | `in: list` | `out: text` | `separator` ("\n"). |
| `regex` | Find matches | `in: text` | `out: list` | `pattern`: a regular expression; outputs every match. |
| `capture` | Capture groups | `in: text` | `group1` … `groupK: list` | `pattern`: one output per capture group, each a list of that group's text in every match. |
| `branch` | Branch | `value: any`, `test: text` | `yes: any`, `no: any` | `pattern` ("(?i)^\\s*yes"): if `test` matches, `value` goes out of `yes`, otherwise out of `no`. |
| `forward` | Pass through | `in: any` | `out: any` | none. |
| `remember` | Remember first | `in: any` | `out: any` | none. Keeps the first value and sends it on every later activation. |
| `for_each` | For each | `in: list` | `out: text` | `limit` (0 = all). Sends each item separately, at most `limit` of them. |
| `separate` | Pick items | `in: list` | `out1` … `outN: text` | `outputs` (2; 1–10): item *i* goes out of `outi`. |
| `read_file` | Read file | `path: text` | `out: text` | none. Reads a file in the agent's workspace (up to 1 MB). |
| `write_file` | Write file | `path: text`, `content: text` | `out: text` (the path) | `append` (false). Creates folders as needed. |
| `list_files` | List files | `path: text` | `out: list` | none. Folder names end with `/`. Use `.` for the workspace itself. |
| `output` | Reply | `in: text` | none | none. Adds its input to the agent's reply. |
| `function` | Function | the function's inputs | the function's outputs | `function`: the id of a function in this loop. See below. |

Regular expressions use Rust's [`regex`](https://docs.rs/regex) syntax. Start
a pattern with `(?i)` to ignore case.

File paths in workspace blocks are always relative to the agent's workspace.
Absolute paths, `..`, and links that point outside the workspace are refused.

## Functions

A **function** is a group of blocks used as one block. Its blocks are linked
to each other as usual; the ports left open become the function's ports.

```json
{
  "id": "do_step",
  "name": "Do the step",
  "description": "",
  "inputs":  [ { "name": "exchange", "label": "Exchange", "to":   { "block": "work", "port": "in" } } ],
  "outputs": [ { "name": "result",   "label": "Result",   "from": { "block": "work", "port": "out" } } ],
  "blocks": [ … ],
  "links": [ … ]
}
```

| Field         | Meaning |
|---------------|---------|
| `id`          | Unique among the loop's functions; letters, digits, `-`, `_`. |
| `name`, `description` | Display text. |
| `inputs`      | Each names a port of the function block (`name`, unique) and the inner input port values go to (`to`). |
| `outputs`     | Each names a port of the function block and the inner output port it carries (`from`). |
| `blocks`, `links` | The function's own blocks and links, with their own ids. |

A block `{ "type": "function", "params": { "function": "do_step" } }` uses it,
anywhere in the loop or inside another function. It behaves like any block:

- it waits until every one of its inputs has a value, takes one from each,
- then runs its blocks as a fresh run: the input values go to their inner
  ports, blocks without inputs start, and everything runs until nothing is
  ready (cycles inside are fine);
- then it sends out everything that reached its outputs, in order. An output
  can produce several values per call, or none.

Each call starts fresh (a *Remember first* block inside forgets between
calls). A function can't use itself, directly or through another function.
In the editor, select some blocks and choose **Make function**: their open
inputs and outputs become the function's ports, and links from the rest of
the loop are moved onto the new block. Run events name blocks inside a
function by path: `<function block id>/<block id>`.

## Validation

Chummy checks a loop when it's opened, edited, imported or run.

**Errors** (the loop can't run): wrong `format` or `version`; bad or duplicate
block ids; unknown block types; bad parameters or invalid regular expressions;
links to missing blocks or ports; links between mismatched kinds; duplicate
links; limits out of range; no starting block; a function that uses itself,
or whose inputs or outputs name missing ports.

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

## Example: 5 step recursive plan

This template is an ordinary loop file:
[`crates/flow/templates/five-step-plan.chummy.json`](../crates/flow/templates/five-step-plan.chummy.json).
It treats the message as a goal and asks for a plan of 5 steps or fewer.
Each step joins one queue (*Steps to do*), and a running exchange holding
the goal and every result so far circulates like a token, so steps are
handled one at a time, in order. For each step it asks the model whether the
step can be done now (by writing, or with a tool). If YES, the *Do the step*
function lets the model work on it, calling tools in a loop, and the result
is shown and added to the exchange. If NO, the *Break it down* function plans
5 or fewer smaller steps, which are shown and join the **back** of the queue,
so the work proceeds breadth-first. When the queue is empty the run ends,
with the running exchange left waiting, which is expected.
