# The Chummy loop format (`chummy.loop`, version 3)

A loop is how an agent thinks: a directed graph of **blocks** that pass text
(and conversations with the model) to each other along **links**, from the
user's message to the agent's reply.
Loops are saved as JSON files. Every object rejects unknown fields, so a valid
file means exactly one thing.

## File

```json
{
  "format": "chummy.loop",
  "version": 3,
  "name": "Simple chat",
  "description": "Answers each message with the recent conversation as context.",
  "limits": { "max_steps": 200, "max_llm_calls": 20, "max_tool_calls": 20 },
  "blocks": [ … ],
  "links": [ … ],
  "functions": [ … ],
  "notes": [ … ]
}
```

| Field         | Type   | Required | Meaning |
|---------------|--------|----------|---------|
| `format`      | string | yes      | Always `"chummy.loop"`. |
| `version`     | number | yes      | Format version. This document describes `3`. Older files still load: version `1` had no comments, labels or notes, and before version `3` a model block (or Find tool call) without `tools` or `network` had none, so those files load with both set to `"off"`. Newer versions are refused, never guessed at. Chummy saves as `3`. |
| `name`        | string | yes      | Display name. |
| `description` | string | no       | Defaults to `""`. |
| `limits`      | object | no       | Safety limits for one run (see below). |
| `blocks`      | array  | yes      | The blocks. |
| `links`       | array  | no       | The links. Defaults to `[]`. |
| `functions`   | array  | no       | Block functions used in this loop (see below). Omitted when empty. |
| `notes`       | array  | no       | Notes on the canvas (see below). Omitted when empty. |

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
  "params": { "prompt": "_INPUT_\n\nWrite the assistant's reply.", "max_tokens": 600 } }
```

| Field      | Type   | Required | Meaning |
|------------|--------|----------|---------|
| `id`       | string | yes      | Unique in the loop. 1–64 characters: letters, digits, `-`, `_`. |
| `type`     | string | yes      | A block type from the table below. |
| `label`    | string | no       | Display name. |
| `position` | object | no       | `{ "x": number, "y": number }` on the editor canvas. Doesn't affect running. |
| `params`   | object | no       | Type-specific parameters. Unknown keys are errors; missing keys take their defaults. |
| `comment`  | string | no       | A note for people, shown on the block. Never sent to the model. Omitted when empty. |

### Link

```json
{ "from": { "block": "answer", "port": "out" }, "to": { "block": "reply", "port": "in" } }
```

A link may have a `"label"` (a short text shown on the path, for people).

A link carries **every** value an output port produces to one input port. The
same link can't appear twice. An output can feed any number of inputs, and an
input can receive from any number of outputs.

### Note

```json
{ "id": "note-1", "text": "Failures don't stop the run…", "position": { "x": 0, "y": 400 },
  "width": 260, "height": 90 }
```

Free-standing text on the canvas, for people. `width` and `height` are
optional. Notes, comments and labels never change what a loop does.

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
2. The model's tools come from two levels of switches, plus the block's
   own setting (see **Tool switches** below). A block whose **Tool call**
   output isn't connected is offered no tools (unless `tool_mode` is `off`),
   so it simply answers.
3. If the model answers, the answer goes out of **Answer**. If it calls a
   tool, the exchange plus the model's request goes out of **Tool call**
   instead, and Answer stays silent. **Reasoning** carries any thinking the
   model showed (`<think>` sections or the provider's reasoning) in both
   cases, possibly empty. Unconnected outputs are harmless.
4. How tools reach the model depends on `tool_mode`:
   - `native` (default): through the model server's tool calling.
   - `prompted`: described in the system message, with the calling format
     (`<tool_call>{"name": …, "arguments": {…}}</tool_call>`). Chummy finds
     the calls in the model's text, so this works with servers or models
     that can't call tools. The exchange is sent as plain text.
   - `off`: the block offers no tools and never fires **Tool call**. The
     loop handles tools itself: `_TOOLS_` in a prompt lists the block's
     tools (`- name(arguments): description`), and a **Find tool call**
     block reads a call out of the answer.
5. Calls are checked first: the tool must be one of the block's, and the
   arguments must match its parameters. Numbers and true/false written as
   text are fixed quietly; anything else is a bad call, as is not calling a
   tool when `tool_choice` is `required`. `on_bad_call` decides what happens:
   `retry` (tell the model what was wrong and ask again, up to `retries`
   times, then stop), `fail` (stop), or `answer` (use the text as the answer).
6. A tool may go online only if Internet access is on in Permissions and
   the tool's own internet switch is on; then the block's `network` decides
   (`inherit`, the default, uses the agent's internet switch).

### Tool switches

Two switches decide which tools a block offers: the **app-wide** one (tools
on in Permissions, and the tool on in Settings → Tools) and the **agent's**
(its tool and internet switches). The app-wide one always applies; nothing in
a loop can get past it. A block follows the agent by default and can override
it, one tool at a time:

| Switch    | Meaning                                                            |
|-----------|--------------------------------------------------------------------|
| `inherit` | What the agent allows (the default, also when the key is left out). |
| `on`      | On for this block, even when the agent's switch is off. The run shows a notice when this happens. |
| `off`     | Never in this block.                                               |

`tools` is one switch for every tool (`"inherit"`, `"on"` or `"off"`), or an
object when single tools differ: `{"all": "off", "each": {"calculator":
"inherit"}}` offers only the calculator, if the agent allows it. `network` is
one switch (`"inherit"`, `"on"` or `"off"`). Old forms still load: a list of
tool names means those tools inherit and every other is off, and `true` /
`false` mean `inherit` / `off`.

Model settings inherit the same way: `max_tokens` and `temperature` left out
use the model connection's own settings (Settings → Models), and `model` left
out uses the agent's model.

A **Run tool** block runs the calls at the end of an exchange in the agent's
sandbox, adds their results as tool messages, and sends the exchange on.
Linking it back into the model block lets the model continue with the
results:

```text
           ┌──────── Run tool ◀─── Tool call ┐
text ─▶ Ask the model ─────────────────────── ┘
                    └─ Answer ─▶ Reply
```

## Failed outputs

Blocks that can fail have an output called `error` (**Failed** in the
editor): `llm`, `run_tool`, `ask_user`, `read_file`, `write_file` and
`list_files`. When one fails and its `error` output is linked (or is a
function's output), the failure message goes out of it as text and the run
carries on. When it isn't linked, the failure stops the run, as always. Run
limits (steps, model calls, tool calls) and the user stopping a run always
stop it.

## Memory

A **Save to memory** block (`store`) keeps its input under a name for the
rest of the run (function calls included) and passes it on. Any prompt,
system message, *Combine* format, *Add to exchange* format or *Ask me*
question can include it as `{{name}}`; a name with nothing saved yet becomes
empty text. **Read memory** (`recall`) sends the saved text out when its
input fires. Names are letters, digits and `_`. Text in memory is the model's
own working notes, so it is never trusted for anything but text.

### Plan outline

A **Plan outline** block (`outline`) keeps a whole plan as a tree of tasks
for the rest of the run, by memory name (`key`, default `plan`), and sends it
out as numbered, indented text each time it runs (also saved to memory, so a
prompt can include `{{plan}}`):

```text
▾ 1. Get ready for the trip
    ✓ 1.1. Book the flights
    ▾ 1.2. Pack
        ○ 1.2.1. Clothes
        ○ 1.2.2. Documents
    ✗ 1.3. Find a pet sitter
```

`action` picks what it does: `start` (input `in`: the goal; starts over),
`add_steps` (inputs `task` and `steps`, a list: adds up to `limit` steps
under the task and marks it broken down ▾) or `mark` (input `task`: sets it
to `status`, `done` ✓, `failed` ✗, `working` ▸ or `pending` ○). Tasks are
found by their text, ignoring a leading `Task:`, `**` and case; with the same
text twice, the first unfinished one in working order (breadth-first) is
used. A task that isn't in the outline is added at the top level. It's text
handling only: nothing runs.

## Run logs

Every run writes a log (see the Logs window): each block step with the
values on its inputs and outputs, each model call (the conversation sent,
reasoning, answer and tool calls), tool calls and results, questions,
notices and replies. Values longer than 6,000 characters are cut in the
middle (the log says how much was left out). Steps inside a function are
named by path and come between the function's start and end.

## Daily token limits

Each model connection has a daily token limit (set in Settings → Models).
Before each call a model block checks it. When the day's tokens are used up,
`on_limit` decides: `ask` (ask the user to allow more today, using the same
mechanism as *Ask me*; refusing stops the run), `wait` (wait until midnight,
then carry on) or `stop` (stop the run).

How tools themselves are written, installed and sandboxed is described in
[tool-format.md](tool-format.md).

## Block types

Ports are written `name: kind`.

| Type | Name in the editor | Inputs | Outputs | Parameters (default) |
|------|-------------------|--------|---------|----------------------|
| `input` | Message | none | `out: text` | none. Sends the chat message. |
| `history` | Conversation | none | `out: text` | `max_messages` (10): recent messages, formatted `User: …` / `Assistant: …`. |
| `fixed` | Fixed text | none | `out: text` | `text` (""). |
| `llm` | Ask the model | `in: any` | `out: text` (Answer), `tool: exchange` (Tool call, only with tools and `tool_mode` not `off`), `reasoning: text`, `error: text` | `prompt` ("_INPUT_"), `system` (""), `model` (unset = the agent's model; otherwise a model connection id), `temperature` (unset = the model connection's setting; 0–2), `max_tokens` (unset = the model connection's longest answer, 1,024 unless changed in Settings → Models; 1–32,768), `tools` ("inherit"; see Tool switches), `tool_mode` ("native", "prompted" or "off"), `network` ("inherit"), `on_limit` ("ask", "wait" or "stop"), `tool_choice` ("auto" or "required"), `on_bad_call` ("retry", "fail" or "answer"), `retries` (2; 0–5). `_TOOLS_` in the prompt or system message lists the block's tools; `{{name}}` includes memory. Text input: `_INPUT_` in the prompt is replaced by it (or it's added after the prompt). Exchange input: the exchange is continued; a non-empty `system` replaces its system message for this call. `<think>…</think>` sections are moved from the answer to Reasoning. |
| `run_tool` | Run tool | `in: exchange` | `out: exchange`, `error: text` | `max_result_chars` (4000), `on_error` ("tell_model" or "fail"), `confirm` (false). Runs the pending tool calls at the end of the exchange and adds their results. Longer results are cut. With `confirm`, each call waits for the user to allow it (see *Asking the user*). |
| `ask_user` | Ask me | `in: any` | `out: text`, `error: text` | `prompt` ("_INPUT_"; `_INPUT_` is replaced by the input, or the input is added after it), `timeout_minutes` (0 = wait as long as it takes; at most 10080), `default_answer` (""). Asks the user and sends the answer on. If the wait times out, `default_answer` is used, or the run stops if it's empty. |
| `exchange` | Start exchange | `in: any` | `out: exchange` | `system` (""), `roles` (["user"]). Text becomes one message with the first role; a list becomes one message per item, with the roles in turn. |
| `augment` | Add to exchange | `exchange: exchange`, `in: any` | `out: exchange` | `role` ("user"), `format` ("_INPUT_"). Adds the text as a message (`_INPUT_` replaced by it); a list adds one message per item. |
| `read_exchange` | Read exchange | `in: exchange` | `last: text`, `all: text`, `calls: text` | `role` ("assistant"; or "user", "tool", "system", "any"). `last` is the last message from that role, `all` the transcript, `calls` the pending tool calls described for people (empty if none). |
| `decline_calls` | Decline tool calls | `in: exchange`, `reason: text` | `out: exchange` | none. Answers each pending tool call with "Not run" and the reason, without running it. |
| `find_call` | Find tool call | `exchange: exchange`, `in: text` | `call: exchange`, `none: text` | `tools` ("inherit"), `network` ("inherit"), as in Tool switches. Finds tool calls in the text and adds them to the exchange for *Run tool*. Only tools the block offers count; text without a usable call goes out of `none`. |
| `outline` | Plan outline | `in: text` (start), `task: text` and `steps: list` (add steps), or `task: text` (mark) | `out: text` | `action` ("start", "add_steps" or "mark"), `status` ("done"), `limit` (5; 0 = all), `key` ("plan"). See *Plan outline*. |
| `store` | Save to memory | `in: text` | `out: text` | `key` ("notes"), `max_chars` (4000; at least 100; longer text keeps its end). |
| `recall` | Read memory | `in: any` | `out: text` | `key` ("notes"). |
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
| `read_file` | Read file | `path: text` | `out: text`, `error: text` | none. Reads a file in the agent's workspace (up to 1 MB). |
| `write_file` | Write file | `path: text`, `content: text` | `out: text` (the path), `error: text` | `append` (false). Creates folders as needed. |
| `list_files` | List files | `path: text` | `out: list`, `error: text` | none. Folder names end with `/`. Use `.` for the workspace itself. |
| `output` | Reply | `in: text` | none | none. Adds its input to the agent's reply. |
| `function` | Function | the function's inputs | the function's outputs | `function`: the id of a function in this loop. See below. |

Regular expressions use Rust's [`regex`](https://docs.rs/regex) syntax. Start
a pattern with `(?i)` to ignore case.

File paths in workspace blocks are always relative to the agent's workspace.
Absolute paths, `..`, and links that point outside the workspace are refused.

## Asking the user

Two things wait for the person using Chummy: an **Ask me** block, and a
**Run tool** call that needs their OK. Both use the same mechanism: the
question appears in the agent's chat (or in the editor's run panel for a test
run, and in the Scheduler for a job), and the run waits until it's answered,
stopped, or an *Ask me* block's time runs out. Scheduled jobs wait too, and
Chummy shows a notification.

A tool call needs the user's OK when:

- its Run tool block has `confirm` on, or
- the tool can use the internet right now and **Ask before tools go online**
  is on in Settings → Permissions (the default), or
- the user set the tool to **Ask me before each use** in Settings → Tools.

A loop can ask for more confirmation, never less: the last two rules come
from the app's settings, not the loop. A refused call goes back to the model
as an error result (`Error: the user didn't allow this tool call: <reason>`),
or stops the run if the block's `on_error` is `fail`.

Every block is text handling, model calls, or work in the agent's folder.
There is no script block: the only code a loop can run is a tool, and tools
only ever run in the WebAssembly sandbox.

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

## Agent loop configuration (`chummy.loopconfig`)

An agent can change any block's parameters without changing the loop file.
The changes are stored in the agent's settings, per loop, and applied when
the agent runs the loop. They can be exported and imported:

```json
{
  "format": "chummy.loopconfig",
  "version": 1,
  "loop": "Chummy assistant",
  "blocks": {
    "plan/plan_model": { "temperature": 0.2 },
    "attempt/try": { "tool_mode": "prompted", "on_limit": null }
  }
}
```

Keys are a block id, or `function id/block id` for a block inside a
function. Each value replaces those parameters; `null` removes one, so the
block's default applies. Settings for blocks the loop doesn't have are
skipped. Every change is checked against the block catalog and the whole
loop is checked again before it's saved, so a configuration can't break a
loop, add blocks, or turn on anything the app or the agent doesn't allow.

## Size limits

A loop may have at most 5,000 blocks and 20,000 links (counting those inside
functions) and 500 functions. Imported files are limited in size too.

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
[`crates/flow/templates/five-step-plan.chummy.json`](../crates/flow/templates/five-step-plan.chummy.json)
(written by `make_templates.py` next to it). It treats the message as a goal.

- Every task, starting with the goal itself, waits in one queue (*Tasks to
  do*). A running exchange (the goal, then each task and its result)
  circulates like a token, so tasks are handled one at a time, in order.
- Each task is **tried first**: the *Attempt the task* function lets the
  model do it, by writing or with tools (calling them as often as it needs).
  A task it can do is never planned; a goal it can do in one go is answered
  straight away.
- If the model replies `TOO BIG`, the *Plan smaller steps* function breaks
  the task into 5 or fewer steps, shows the plan, and the steps join the
  **back** of the queue, so the work goes breadth-first.
- Every result is shown and added to the running exchange. When the queue
  is empty the run ends, with the running exchange left waiting, which is
  expected.

**5 step recursive plan verify**
([`five-step-plan-verify.chummy.json`](../crates/flow/templates/five-step-plan-verify.chummy.json))
adds a *Verify* function: the model writes a yes/no check question for an
answer ("Does this answer make sense given the request?") and then answers
it. Results that fail their check are broken down like a task that's too
big; plans that fail are made again once, with the reason. It's slower and
more careful.

## Example: Chummy assistant (the default)

[`assistant.chummy.json`](../crates/flow/templates/assistant.chummy.json)
grows the verify planner into the loop Chummy recommends. Every part is a
function with comments, labels and notes, so it reads well in the editor.

- **Clarify the request.** Before anything else the model decides whether
  the request is clear. If it isn't, an *Ask me* block asks you one
  question, and your answer joins the request. If you don't answer within
  the time limit, it carries on with what it has.
- **Working notes.** The running exchange's system message includes
  `{{notes}}`, read fresh at every model call. After each result and each
  plan, *Update the notes* asks the model to rewrite its notes (what's done,
  what was learned, what's left), saved with *Save to memory*. If that model
  call fails, the old notes are kept.
- **Attempt the task.** The model tries each task, with tools. Before a
  call runs, *Check the tool call* asks the model whether that call is right
  for the task; a call that fails the check is declined (*Decline tool
  calls*) with the reason and the model tries again. If the model is stuck
  it can answer `QUESTION: …`, which asks you and passes your answer back.
- **Errors don't stop the run.** The *Failed* outputs of the model and tool
  blocks are linked: a failed tool call goes back to the model as an error
  message, and a failed model call becomes "I couldn't finish this task: …"
  so the remaining tasks still run.
- **Output.** Each result is shown as it's done (`✓ Task: …` with the
  result). Each breakdown shows the **whole plan** from a *Plan outline*:
  every task so far, nested by level, with finished ones ticked. The model
  sees the plan too, as `{{plan}}` in its attempt prompt.
- **Plan and verify.** As in the verify planner, tasks that are too big are
  broken into 5 or fewer steps, and every result and plan is checked.
