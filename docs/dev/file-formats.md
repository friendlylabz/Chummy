# File formats

The three files Chummy reads and writes that people may share or edit:

- [Loops](#loops) (`chummy.loop`, version 3)
- [Tools](#tools) (one PEP 723 Python file)
- [Scheduled jobs](#scheduled-jobs) (`chummy.job`, version 1)

## Loops

The loop format: `chummy.loop`, version 3.

A loop is how an agent thinks: a directed graph of **blocks** that pass text
(and conversations with the model) to each other along **links**, from the
user's message to the agent's reply.
Loops are saved as JSON files. Every object rejects unknown fields, so a valid
file means exactly one thing.

### File

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

#### Limits

| Field           | Default | Range     | Meaning |
|-----------------|---------|-----------|---------|
| `max_steps`     | 200     | 1–10,000  | Most block runs in one run of the loop. |
| `max_llm_calls` | 20      | 1–1,000   | Most model calls in one run of the loop, retries included. |
| `max_tool_calls`| 20      | 1–1,000   | Most tool calls in one run of the loop. |

Blocks inside functions count toward the same limits.

A run that reaches either limit stops and reports it, so a cycle can't run forever.

#### Block

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

#### Link

```json
{ "from": { "block": "answer", "port": "out" }, "to": { "block": "reply", "port": "in" } }
```

A link may have a `"label"` (a short text shown on the path, for people).

A link carries **every** value an output port produces to one input port. The
same link can't appear twice. An output can feed any number of inputs, and an
input can receive from any number of outputs.

#### Note

```json
{ "id": "note-1", "text": "Failures don't stop the run…", "position": { "x": 0, "y": 400 },
  "width": 260, "height": 90 }
```

Free-standing text on the canvas, for people. `width` and `height` are
optional. Notes, comments and labels never change what a loop does.

### How a loop runs

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
- A chat run can **pause** between blocks (`run_with` with a pause flag):
  it stops before taking the next ready block, in whatever function call it
  is in, and hands back a `RunState` (version 1): each graph's input
  queues, ready queue and *Remember* values, from the loop down to the
  innermost function call, with the function block each one was running;
  plus memory, plan outlines, replies, counts and token figures. Given that
  state, the same loop carries on exactly where it stopped. A block waiting
  on the person (*Ask me*, a tool approval, a daily limit) is cut short and
  runs again from the same inputs on resuming, unless a tool already ran in
  it (then the pause waits for it to finish). A model that can't be
  reached (`LlmFailure::unavailable`: no connection, a broken stream, HTTP
  408, 429, 502, 503 or 504) pauses a run that can pause, at that model
  block, which asks again on resuming; the call isn't counted. Runs that
  can't pause (jobs, editor test runs) fail as before. A state that doesn't
  fit the loop exactly (blocks, ports, schedule, function calls) is refused.
- A resumed run is **the same run**: it keeps its run id, so its run log
  carries on in the same file (after a "Resumed after a pause." notice; the
  last `end` entry is the outcome) and its chat messages stay one response.
  The "Paused" notice messages are deleted on resuming, replies sent before
  the pause aren't repeated, and on finishing every message of the run gets
  the run's figures for the whole run, time before the pause included
  (`chat:run_updated` tells windows).

### Exchanges and tools

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

#### Tool switches

Two switches decide which tools a block offers: the **app-wide** one (tools
on in Security and Privacy, and the tool on in Settings → Tools) and the
**agent's** (its tool and internet switches). The app-wide one always applies;
nothing in a loop can get past it. A block follows the agent by default and
can only **turn things off** for itself (the editor shows this as "Disable
tools"), never on, so a loop file from someone else can't widen what an agent
may do:

| Switch    | Meaning                                                            |
|-----------|--------------------------------------------------------------------|
| `inherit` | What the agent allows (the default, also when the key is left out). |
| `off`     | Never in this block.                                               |

`tools` is one switch for every tool (`"inherit"` or `"off"`), or an object
when single tools differ: `{"all": "inherit", "each": {"run_shell": "off"}}`
disables only run_shell; `{"all": "off", "each": {"calculator": "inherit"}}`
disables everything but the calculator (which is still offered only if the
agent allows it). `network` is one switch (`"inherit"` or `"off"`). Old forms
still load: `"on"` (from before blocks became disable-only) reads as
`"inherit"`, a list of tool names means those tools inherit and every other is
off, and `true` / `false` mean `inherit` / `off`.

Model settings inherit the same way: `max_tokens` and `temperature` left out
use the model connection's own settings (Settings → Models, with the agent's
changes), and `model` left out uses the agent's model. The block's
`generation` object holds the rest, each key working like an agent's model
settings: left out inherits, `null` isn't sent (the server's default), and a
value is used. For example `"generation": { "reasoning_effort": "high",
"top_p": null }` thinks harder in this block only and sends no top P. Keys:
`reasoning_effort` ("none", "minimal", "low", "medium", "high"), `top_p`
(0–1), `top_k` (1–1000), `min_p` (0–1), `presence_penalty` and
`frequency_penalty` (−2–2), `repeat_penalty` (0–10), `seed`, `stop` (up to 4
strings; `[]` sends none). Settings the model's kind of server doesn't take
are never sent. The editor shows `temperature`, `max_tokens` and
`generation` together as the block's **Generation settings**.

A **Run tool** block runs the calls at the end of an exchange in the agent's
sandbox, adds their results as tool messages, and sends the exchange on.
Linking it back into the model block lets the model continue with the
results:

```text
           ┌──────── Run tool ◀─── Tool call ┐
text ─▶ Ask the model ─────────────────────── ┘
                    └─ Answer ─▶ Reply
```

### Failed outputs

Blocks that can fail have an output called `error` (**Failed** in the
editor): `llm`, `run_tool`, `ask_user`, `read_file`, `write_file` and
`list_files`. When one fails and its `error` output is linked (or is a
function's output), the failure message goes out of it as text and the run
carries on. When it isn't linked, the failure stops the run, as always. Run
limits (steps, model calls, tool calls) and the user stopping a run always
stop it.

`llm` and `run_tool` also have **Failed (exchange)** (`failed_exchange`):
when the block was given an exchange, the failure goes out of it as that
exchange with the failure added as its last message, a user message reading
"(The last step failed: …)". Linking either failure output catches the
failure. Give it to a model to explain what went wrong, and use **Drop last
messages** (`drop_last`) to take the failure back out before trying again
(the Chummy assistant's *Report what went wrong* does both).

### Chat output

Each run is **one response** in the chat: everything **Add to chat output**
blocks (`output`) send, in order, with any question the run asked and your
answer, and a notice if it stopped or failed. It has one set of figures
(time, calls, steps, tokens), one *Agent logs* and one delete. **Final
chat output** (`final`) adds its input and ends the run as completed: blocks
still waiting don't run. Use it for the final answer and *Add to chat
output* for anything said along the way.

### Memory

A **Save to memory** block (`store`) keeps its input under a name for the
rest of the run (function calls included) and passes it on. Any prompt,
system message, *Combine* format, *Add to exchange* format or *Ask me*
question can include it as `{{name}}`; a name with nothing saved yet becomes
empty text. **Read memory** (`recall`) sends the saved text out when its
input fires. Names are letters, digits and `_`. Text in memory is the model's
own working notes, so it is never trusted for anything but text.

#### Plan outline

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

### Run logs

Every run writes a log (see the Logs window): each block step with the
values on its inputs and outputs, each model call (the conversation sent,
reasoning, answer and tool calls), tool calls and results, questions,
notices and replies. Values longer than 6,000 characters are cut in the
middle (the log says how much was left out). Steps inside a function are
named by path and come between the function's start and end.

### Daily token limits

Each model connection has a daily token limit (set in Settings → Models).
Before each call a model block checks it. When the day's tokens are used up,
`on_limit` decides: `ask` (ask the user to allow more today, using the same
mechanism as *Ask me*; refusing stops the run), `wait` (wait until midnight,
then carry on) or `stop` (stop the run).

### Context length and turn figures

Each model connection has a context length: the one the server reports
(found when the connection is saved or tested; OpenAI's own API doesn't
report one, but vLLM, SGLang, llama.cpp, LM Studio, Ollama, OpenRouter,
Gemini and Anthropic do), or one you set (never more than the reported
one), or 8,192 tokens if nothing says. Its *Make room at* point (80% of
the context unless set, never more than it) is what *Context full?* blocks
measure against.

Every run records three estimates (about four characters a token), shown
under its reply in chat and in the run's log: the tokens the turn added to
context (your message, every model answer with its tool calls, every tool
result), how full the context of the latest model call was (what it was
sent plus its answer, against that model's context length), and the
model's daily token use when the reply came. The reply also keeps how long
the run took (start to finish, to a tenth of a second), shown first; while
a run works, a timer shows next to the step it's on. Both work for any loop.

How tools themselves are written, installed and sandboxed is described in
[Tools](#tools).

### Block types

Ports are written `name: kind`.

| Type | Name in the editor | Inputs | Outputs | Parameters (default) |
|------|-------------------|--------|---------|----------------------|
| `input` | Message | none | `out: text` | none. Sends the chat message. |
| `history` | Conversation | none | `out: text` | `max_messages` (10): recent messages, formatted `User: …` / `Assistant: …`. |
| `fixed` | Fixed text | none | `out: text` | `text` (""). |
| `llm` | Ask the model | `in: any` | `out: text` (Answer), `tool: exchange` (Tool call, only with tools and `tool_mode` not `off`), `reasoning: text`, `error: text`, `failed_exchange: exchange` | `prompt` ("_INPUT_"), `system` (""), `model` (unset = the agent's model; otherwise a model connection id), `temperature` (unset = the model connection's setting; 0–2), `max_tokens` (unset = the model connection's longest answer, 1,024 unless changed in Settings → Models; 1–32,768), `generation` (unset = all inherited; reasoning effort, sampling, penalties, seed and stop sequences; see the note on model settings under Tool switches), `tools` ("inherit"; see Tool switches), `tool_mode` ("native", "prompted" or "off"), `network` ("inherit"), `on_limit` ("ask", "wait" or "stop"), `tool_choice` ("auto" or "required"), `on_bad_call` ("retry", "fail" or "answer"), `retries` (2; 0–5), `reminder` (""; exchange input only: a note sent last, as a user message, for this call only and never kept in the exchange; `{{name}}` reads memory; empty after filling sends nothing). `_TOOLS_` in the prompt or system message lists the block's tools; `{{name}}` includes memory. Text input: `_INPUT_` in the prompt is replaced by it (or it's added after the prompt). Exchange input: the exchange is continued; a non-empty `system` replaces its system message for this call. `<think>…</think>` sections are moved from the answer to Reasoning. |
| `run_tool` | Run tool | `in: exchange` | `out: exchange`, `error: text`, `failed_exchange: exchange` | `max_result_chars` (50000), `on_error` ("tell_model" or "fail"), `confirm` (false). Runs the pending tool calls at the end of the exchange and adds their results. Longer results are cut, and so are results over about half the model's context (at least 12,000 characters). With `confirm`, each call waits for the user to allow it (see *Asking the user*). |
| `ask_user` | Ask me | `in: any` | `out: text`, `error: text` | `prompt` ("_INPUT_"; `_INPUT_` is replaced by the input, or the input is added after it), `timeout_minutes` (0 = wait as long as it takes; at most 10080), `default_answer` (""). Asks the user and sends the answer on. If the wait times out, `default_answer` is used, or the run stops if it's empty. |
| `exchange` | Start exchange | `in: any` | `out: exchange` | `system` (""), `roles` (["user"]). Text becomes one message with the first role; a list becomes one message per item, with the roles in turn. |
| `augment` | Add to exchange | `exchange: exchange`, `in: any` | `out: exchange` | `role` ("user"), `format` ("_INPUT_"). Adds the text as a message (`_INPUT_` replaced by it); a list adds one message per item. |
| `drop_last` | Drop last messages | `in: exchange` | `out: exchange` | `count` (1; 1–50). Takes that many messages off the end, never the system message at the start. |
| `read_exchange` | Read exchange | `in: exchange` | `last: text`, `all: text`, `calls: text`, `newest: text` | `role` ("assistant"; or "user", "tool", "system", "any"). `last` is the last message from that role with any text (messages that only call tools are skipped), `newest` the newest message from that role even if it only calls tools (then it's empty), `all` the transcript (without the system message when `system_in_transcript` is false; default true), `calls` the pending tool calls described for people (empty if none). |
| `context_check` | Context full? | `in: exchange` | `room: exchange`, `full: exchange` | `model` (optional; the agent's by default), `percent` (optional, 1–100). Estimates the exchange's tokens (about four characters a token; system messages counted as they'd be sent, memory filled in) and sends it out of `full` when that reaches the model's compaction point (Settings → Models → *Make room at*, 80% of the context by default) or `percent` of its context, otherwise out of `room`. Going full adds a notice to the run. |
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
| `output` | Add to chat output | `in: text` | none | none. Adds its input to the run's response in the chat; the run carries on. |
| `final` | Final chat output | `in: text` | none | none. Adds its input to the response and ends the run as completed (anything still waiting is dropped). |
| `pane` | Pane output | `in: text` | none | none. Adds its input as one line (whitespace folded, at most 1,000 characters; blank input shows nothing) to the run's **pane**, a live log in the Sessions panel (the run's text button), newest first. Never part of the reply or the conversation. A run's pane keeps its newest 500 lines. |
| `stop` | Stop with error | `in: text` (the reason) | none | none. Ends the whole run as failed, with its input as the reason; anything still waiting is dropped. It has no *Failed* output, so it can't be caught. |
| `function` | Function | the function's inputs | the function's outputs | `function`: the id of a function in this loop. See below. |

Regular expressions use Rust's [`regex`](https://docs.rs/regex) syntax. Start
a pattern with `(?i)` to ignore case.

File paths in workspace blocks are always relative to the agent's workspace.
Absolute paths, `..`, and links that point outside the workspace are refused.

### Asking the user

Two things wait for the person using Chummy: an **Ask me** block, and a
**Run tool** call that needs their OK. Both use the same mechanism: the
question appears in the agent's chat (or in the editor's run panel for a test
run, and in the Automation menu for a job), and the run waits until it's answered,
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

### Functions

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

### Agent loop configuration (`chummy.loopconfig`)

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

### Paused chat runs (`paused/<agent>/<chat>.run`)

Not a file anyone writes or shares: Chummy seals it, and only Chummy opens
it. It's `CHUMMY-PAUSED-RUN\x01`, a 24-byte nonce, then XChaCha20-Poly1305
ciphertext of a `chummy.paused-run` v1 JSON document (agent and chat ids,
the chat title, the agent's workspace path, the loop as it ran with the
agent's block settings applied, the run's inputs, its `RunState`, when and
why it paused; the run id, the time it had run so far, and the ids of the
chat's "Paused" notice messages to remove on resuming). The associated data binds it to its agent and chat. Each
pause has its own random key, kept only in the OS keychain as
`paused-run:<agent>:<chat>`; resuming deletes the key, then the file. There's
no import or export, and a file that doesn't open is deleted. See
[Security: paused runs](security.md#paused-runs).

### Size limits

A loop may have at most 5,000 blocks and 20,000 links (counting those inside
functions) and 500 functions. Imported files are limited in size too.

### Fingerprints and trusted loops

A loop's **fingerprint** is the SHA-256 of the file Chummy would save for it
(`LoopFile::to_json` with the current format version), so spacing and key
order in a file don't change it, but any change to blocks, links, text,
names, notes or positions does. Chummy runs and opens only loops whose
fingerprint is trusted, and reviews any other before trusting it; see
[Security and threat model → Loops from elsewhere](security.md#loops-from-elsewhere-review-and-trusted-loops-after-v012).

### Validation

Chummy checks a loop when it's opened, edited, imported or run.

**Errors** (the loop can't run): wrong `format` or `version`; bad or duplicate
block ids; unknown block types; bad parameters or invalid regular expressions;
links to missing blocks or ports; links between mismatched kinds; duplicate
links; limits out of range; no starting block; a function that uses itself,
or whose inputs or outputs name missing ports.

**Warnings** (the loop runs, probably not as intended): no *Reply* block; an
input port with nothing connected, so its block never runs; a block that can't
be reached from any starting block.

### Example: draft and self-check

A small loop you can build yourself (it used to ship as a template). It
drafts a reply, asks the model to check it, and improves it until the check
says YES (or a 12-model-call limit is reached).

```text
Conversation ─┐
              ├─ Combine ─ Draft ─┬──────────────── Branch.value ── yes ─ Reply
Message ──────┘                  └─ Check ─ Branch.test
                                    ▲          └── no ─ Improve ─┬─ Branch.value
                                    └────────────────────────────┘
```

### Example: Terminal assistant

[`terminal.chummy.json`](../../crates/flow/templates/terminal.chummy.json)
(written by `make_templates.py` from `terminal.py`) is the Chummy assistant
below with other prompts and one difference in tools: its Work block's tools
are a per-tool map that turns everything off except the shell tools,

```json
"tools": { "all": "off", "each": { "fetch_page": "inherit", "run_command": "inherit", "run_shell": "inherit" } }
```

and *Show what it says* also sends the first line of each `command` argument
to a Pane output block (Read exchange's Calls output → Capture groups →
Join → Combine `$ _INPUT1_`).

### Example: Chummy assistant (the default)

[`assistant.chummy.json`](../../crates/flow/templates/assistant.chummy.json)
(written by `make_templates.py` from `assistant.py`) is the loop Chummy
recommends: no separate judges or planners, just one model working with
tools until it's done, the way a coding agent's core loop does (it's
modelled on the basic behaviour of Claude Code's query loop). Every part is
a function with comments, labels and notes, so it reads well in the editor.

1. **Get ready.** Saves the request (`{{request}}`) and the conversation
   before it (`{{conversation}}`), reads `CHUMMY.md` from the top of the
   agent's folder into `{{instructions}}` (your instructions for this
   folder, like a project instruction file), lists the folder's top level
   (first 60 entries) into `{{folder}}`, and starts `{{todos}}` and
   `{{progress}}`, and `{{next}}` ("(not decided yet)").
2. **Decide what's next** steers the work. Before every *Work* call (the
   first one, after each round of tool results, and after being sent
   back), a small extra model call (no tools, temperature 0) gets the
   request, `{{todos}}`, the step until now (`{{next}}`) and the newest
   2,500 characters of the work, and decides the ONE next step (the first
   unchecked to-do if there is one) as a direct instruction of at most 25
   words, or "Give the user the final answer now." Its first line goes to
   a *Pane output* block (so the run's pane in Sessions shows each step,
   newest first) and is saved as `{{next}}`. *Work*'s system message names
   it under "Your current step" (work on this step only, no side tasks),
   and *Work*'s **Reminder** sends it as the very last message ("Your step
   now: …", marked as the agent's own planning, not the user), because
   small models follow the last thing they read. The reminder isn't kept
   in the conversation. If the call fails or decides nothing, `{{next}}`
   says to carry on with the first unchecked to-do.
3. **Agent loop.** The exchange starts with a system message that tells
   the model how to work (say what you're about to do,
   read before changing, don't repeat refused calls, ask before anything
   hard to undo, ask one question if unclear, check your work, report
   honestly) and holds the to-do list, folder, instructions and
   conversation, filled in fresh at every call. Each answer with tool calls
   goes through *Show what it says* (the text next to the calls is shown to
   you, using Read exchange's **Newest message** so older text isn't
   repeated), *Run tool* (your approvals apply; errors and refusals go back
   to the model) and *Make room*, then back to *Work*.
4. **Planning** is the model's own checklist. Whenever it writes lines like
   `- [ ] step` / `- [x] step`, *Keep the to-do list* saves them as
   `{{todos}}`, replacing the old list; the model sees the current list in
   every call. Simple requests skip it.
5. **Make room.** After each round of tool results the transcript (without
   the instructions) is saved as `{{progress}}` (newest 8,000 characters).
   A *Context full?* block then estimates the exchange's tokens, system
   message included; once they reach the model's compaction point
   (Settings → Models → *Make room at*: 80% of the context length the
   server reports, unless you change it), the model summarises the work,
   you see a short note, and the work carries on in a fresh exchange: the
   same instructions, then the summary.
6. **Done?** An answer without tool calls ends the loop: one that ends with
   a question waits for your reply (the run ends; your next message carries
   on); an empty one, or one given while `{{todos}}` still has unchecked
   steps, is shown and the model is sent back to carry on (in a fresh
   exchange with the request, `{{progress}}` and the list), twice at most
   per message; otherwise it's the final answer. Delete *Done?* to get the
   plain "stop when the model stops" behaviour.

7. **Report what went wrong.** When *Work* fails (a server error, or tool
   calls still malformed after the bad-call retries) or *Run tool* fails,
   their *Failed (exchange)* goes here: the model (no tools) is given the
   exchange with the failure at its end and says in a sentence or two what
   went wrong and what it will do differently. That's added to the chat
   ("Something went wrong: … Trying again.") and the pane, and becomes
   `{{next}}` ("The last try failed: …"). Then *Drop last messages* takes
   the failure out, any tool calls left waiting are answered as not run,
   and the exchange goes back to *Decide what's next* to try again. After
   3 failures in one run it gives up: "I couldn't finish: …" and *Stop with
   error* end the run as failed. If the report itself fails, the error is
   shown instead.

The final answer goes to *Final chat output*, which ends the run. Limits
are 10,000 steps, 400 model calls (most turns make a short *Decide what's
next* call before *Work*) and 150 tool calls per run.


## Tools

A tool is **one Python file**. Chummy runs it in a WebAssembly sandbox: the
official CPython build for WASI, under Wasmtime. There is no way to run a
tool outside the sandbox.

Chummy ships with a set of built-in tools, installed when you set up tools in
Settings → Tools. After an update, Chummy installs new built-ins and
reinstalls changed ones when it starts (switches are kept; new tools start
off). Every tool is off until you switch it on, and the Tools
permission must be on too. Installing tools from files isn't offered in the
app yet; the installer below is what it will use.

### The file

```python
# /// script
# requires-python = ">=3.14"
# dependencies = ["beautifulsoup4==4.13.4"]
#
# [tool.chummy]
# name = "fetch_page"
# title = "Fetch web page"
# version = "1.0.0"
# network = true
# timeout = 60
# ///
"""Fetches a web page and returns its readable text."""

from bs4 import BeautifulSoup

import chummy


@chummy.tool
def fetch_page(url: str, max_chars: int = 20000) -> str:
    """Get the readable text of a web page.

    url: the full address, starting with https://
    max_chars: stop after this many characters
    """
    r = chummy.http.get(url).raise_for_status()
    ...
```

#### Header

The header is a standard [PEP 723](https://peps.python.org/pep-0723/) inline
metadata block. Chummy reads it **without running the file**, so everything
you agree to is known before anything is installed.

| Field | Meaning |
|-------|---------|
| `requires-python` | Standard. Must allow the sandbox's Python (3.14). |
| `dependencies` | Standard. Plain PyPI requirements only: no URLs, paths or `pkg @ …` references. |
| `[tool.chummy] name` | The name the model calls: starts with `a`–`z`, then `a`–`z`, `0`–`9`, `_`; 48 characters at most. |
| `title` | Display name (defaults to the name). |
| `version` | Shown in Settings. |
| `network` | `true` if the tool needs the internet. It still can't go online until the user allows it. |
| `timeout` | Seconds a call may run (default 30, at most 300). |
| `needs` | What the tool needs from the sandbox beyond Python. `["programs"]` means it starts other programs (`subprocess`), so it only runs in the Container or OpenShell sandbox; in Native it's shown greyed out and never offered. Leave it out otherwise. See [Security and threat model → Sandboxes](security.md#sandboxes-where-tools-run). |
| `risky` | `true` for a tool that can do a lot of damage inside the workspace in one call (like `run_command`). Settings shows it as **Potentially unsafe**, switching it on asks for confirmation, and it then asks before each use until you turn that off. It is still sandboxed like every other tool. |

Unknown keys in `[tool.chummy]` are errors. Other tools' tables are ignored.
The names of built-in tools are reserved.

#### The function

Exactly one function is marked `@chummy.tool`. The model sees:

- **description**: the first paragraph of its docstring (keep it to one
  sentence for small models);
- **parameters**: from the signature. Types `str`, `int`, `float`, `bool`,
  `list[...]` and `Literal[...]` are supported; parameters with defaults are
  optional; `name: help` lines in the docstring become each parameter's help.

The function returns text (or anything JSON can hold). Raise
`chummy.ToolError("…")` with a short message when something's wrong ("File
not found: notes.md"); the model sees it as the result and can correct
itself. Other exceptions are reported as `TypeName: message`, without the
traceback. What the tool prints goes to its log (the first 64 KB are kept),
not to the model. Tool functions must be ordinary functions, not `async`,
and must return rather than call `sys.exit()`.

The model's arguments are checked against the parameters before the call:
numbers and true/false written as text are converted, `null` for an
optional parameter means its default, and anything else that doesn't fit is
explained to the model so it can try again.

#### The `chummy` module

| Name | What it does |
|------|--------------|
| `@chummy.tool` | Marks the function the model calls. |
| `chummy.ToolError` | An error message for the model. |
| `chummy.workspace_path(p)` | Turns a path the model gave into one inside the workspace; refuses absolute paths and `..`. |
| `chummy.WORKSPACE` | Where the workspace is (`/workspace`, or the copy's folder in OpenShell); also the current folder. |
| `chummy.http.get(url, headers=None)`, `chummy.http.post(url, data=None, json=None, headers=None)` | Web requests made by the app on the tool's behalf. Return a `Response` with `status`, `url`, `headers`, `text`, `ok`, `json()`, `raise_for_status()`. |

### The sandbox

Tool calls run in the sandbox chosen in Security and Privacy: Native (below,
the default), Container or OpenShell. Each one gives the tool the same view
(the workspace, the tool, Chummy's runner, no network of its own); see
[Security and threat model → Sandboxes](security.md#sandboxes-where-tools-run). Inside a sandbox that copies the workspace
(OpenShell), `chummy.WORKSPACE` says where the copy is, so use it rather than
writing `/workspace` yourself.

In the **Native** sandbox each call gets a fresh WebAssembly instance that can
see exactly:

| Path | What | Access |
|------|------|--------|
| `/workspace` | the agent's workspace folder | read/write |
| `/tool` | the tool file and its packages | read-only |
| `/lib` | Python's standard library | read-only |
| `/chummy` | Chummy's runner and helper module | read-only |

Nothing else on the computer exists for it. It gets no environment variables
from the computer (only `TZ`, the user's time zone). WASI preview 1 has no
sockets, so a tool has no network of its own: `chummy.http` sends the request
to the app, which makes it only if **all** of these hold:

1. the tool's header says `network = true`;
2. Internet access is on in Security and Privacy;
3. the tool's own internet switch is on in Settings → Tools (turning it on
   asks for confirmation);
4. the address is `http`/`https` and doesn't point to this computer or the
   local network (loopback, private, link-local, carrier-grade NAT and
   similar ranges are refused, after resolving the name, and again at every
   redirect). The connection then goes to the addresses that were checked,
   so a second DNS answer can't point it somewhere else.

Responses are cut at 2 MB. Each call has a time limit (the header's
`timeout`) and a 512 MB memory limit.

### Installing

Installing a tool (built-in tools are installed this way):

1. Read and check the header (no code runs).
2. Check `requires-python` against the sandbox's Python.
3. Resolve the dependencies to a lock: a standard
   [PEP 751](https://peps.python.org/pep-0751/) `pylock.toml`. Built-in tools
   ship with theirs; for others, Chummy asks `uv` to resolve them for a
   WebAssembly platform, so only wheels without compiled code qualify.
4. Download each wheel, check its SHA-256 against the lock, and unpack it.
   Only pure-Python wheels (`…-none-any.whl`) are accepted, and files with
   compiled code are refused even in a mislabelled wheel. Nothing from a
   package runs while installing. A lock with environment markers, or for
   another Python version, is refused: locks for the sandbox are resolved
   for exactly one platform.
5. Load the tool once **inside the sandbox** to read its description and
   parameters.
6. Write `manifest.json`. The new version replaces the old one only when
   everything succeeded.

Installed tools live in the Chummy folder under `tools/installed/<name>/`
(`tool.py`, `pylock.toml`, `site-packages/`, `manifest.json`). Before every
call Chummy checks `tool.py` still matches the checksum recorded when it was
installed, and that it was installed for the current Python (after a Python
update, tools are reinstalled). Installs run one at a time.

### Built-in tools

| Tool | What it does | Internet |
|------|--------------|----------|
| `read_file` | Reads a text file, numbered lines, in chunks: `line_number` (where to start, default 1) and `max_lines` (default 1000, at most 5000). Long lines are cut at 2,000 characters and a call returns at most 40,000; the result says how to read on. | no |
| `write_file` | Creates or replaces a whole file (or appends with `append`). Says how many lines it wrote and what it replaced. | no |
| `edit_file` | Changes part of a file: replaces `old_text`, copied exactly from the file, with `new_text`, like Claude Code's Edit tool. `old_text` must match once (or set `replace_all`); otherwise the error lists where it matched or the closest line. Forgives `read_file`'s line-number prefixes and trailing spaces, keeps Windows line endings, and shows the changed lines afterwards. | no |
| `list_files` | Lists a workspace folder. | no |
| `search_files` | Searches files' contents, like grep: plain text or a `regex`, `file_glob` (e.g. `*.py`), `ignore_case`, `context_lines`, `max_results` (default 200; at most 30,000 characters). Answers `file:line: text`. Skips `.git`, `node_modules`, build folders, hidden files and files over 2 MB. | no |
| `find_files` | Finds files by name with a glob (`*.md`, `src/**/*.ts`), newest first, up to 1,000. | no |
| `run_command` | **Potentially unsafe.** Runs a shell-style command line in the workspace (see below). | no |
| `run_shell` | **Potentially unsafe.** Runs a real `sh -c` command in the workspace, with whatever programs the sandbox has. `needs = ["programs"]`: Container or OpenShell only. Output is cut to 30,000 characters and ends with the exit code; 110-second limit; no host environment. | no |
| `calculator` | Works out arithmetic exactly (no `eval`). | no |
| `current_time` | Today's date and the time, in any time zone (uses `tzdata`). | no |
| `fetch_page` | Gets the readable text of a web page (uses `beautifulsoup4`). | yes |

#### `run_command`

Models are used to working in a shell, so Chummy has a command tool. It can't
be a real bash: Chummy never runs anything outside the WebAssembly sandbox, and
there's no shell inside it. Instead `run_command` is a small shell written in
Python that runs in the same sandbox as every other tool, in the agent's
workspace, with no network:

- **Syntax:** quotes (with `\` escapes), globs (`*.py`, `**/*.md`), braces
  (`{a,b}`, `{1..5}`), pipes `|`, `&&`, `||`, `;` or new lines between
  commands, `\` at the end of a line, `#` comments, redirects `<`, `>`, `>>`,
  `2>`, `2>>`, `2>&1`, `2>/dev/null`, `&>`, `>&2`, heredocs (`<<EOF`,
  `<<'EOF'`, `<<-EOF`), here-strings (`<<<`), `$?`, `set -e`,
  `set -o pipefail`, `exit N`. Not supported, with an error that says what to
  use instead: variables, `$(…)`, backticks, `for`/`while`/`if`, functions,
  `( )` subshells, `<(…)` and background jobs (`&`).
- **Commands:** `ls`, `cat`, `head`, `tail`, `wc`, `grep`/`egrep`/`fgrep`
  (with `-A/-B/-C`, `-o`, `-q`, `-m`, `-r --include`…), `find` (tests, `!`,
  `-o`, `-exec … {} \;` / `{} +`, `-delete`), `sed` (s with flags and
  `\1`/`&`/`\U`, d, p, q, a/i/c, y, =, n/N/D/P, hold space, labels and
  branches, `{ }`, ranges, `-n`, `-E`, `-i`), `awk` (a POSIX awk interpreter:
  fields, patterns and ranges, BEGIN/END, arrays, control flow, user
  functions, printf, the string and math functions; no `getline`,
  `system()` or pipes), `sort` (`-k`, `-t`, `-n`, `-h`, `-V`…), `uniq`,
  `cut`, `tr` (classes like `[:upper:]`), `paste`, `column`, `nl`, `tac`,
  `rev`, `diff`, `echo`, `printf`, `xargs` (runs this shell's commands),
  `tee`, `seq`, `tree`, `du`, `mkdir`, `rmdir`, `rm`, `cp`, `mv`, `touch`,
  `cd`, `pwd`, `basename`, `dirname`, `realpath`, `test`/`[`, `which`,
  `type`, `command -v`, `md5sum`/`sha1sum`/`sha256sum`/`sha512sum`,
  `base64`, `date`, `sleep`, `true`, `false`, `chmod` (does nothing),
  `bash -c`/`sh -c`/`bash script.sh`/`./script.sh`, and `python`/`python3`
  (a script, `-c`, `-m` or stdin), which runs in the sandbox's own Python.
  `help` lists them and `help NAME` (or `NAME --help`) shows one's options.
  An unknown command gets exit code 127 and a pointer to `help`; `curl`,
  `wget`, `npm`, `jq`, `vim` and the like say what to use instead. `pip`
  can't install anything, and `ln` is refused (a link could point outside).
- **Paths** stay inside the workspace; anything else is refused. `rm -r` on
  the workspace itself is refused.
- **Result:** the output (stdout and stderr), cut to 30,000 characters
  (beginning and end kept), then `[exit code N]`.

It is marked `risky`: inside the workspace, one command can delete or change
many files, so switching it on asks first and every call asks you by default.


## Scheduled jobs

The job format: `chummy.job`, version 1.

A job runs one agent's loop, started with a message, on a schedule or when
files change in the agent's folder. Jobs live in the Chummy folder as
`jobs/<id>.json`, and can be exported and imported. The same file works in
the Chummy app and in chummyd (see the [roadmap](roadmap.md#chummyd-the-background-job-service)).

A job never grants anything. It names an agent, and runs with exactly that
agent's folder, model and permissions; it can't do anything the agent can't.
Imported jobs always arrive switched off.

### File

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

### Triggers

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

### Placeholders

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

### How jobs run

- A job runs at most once at a time. A scheduled time that comes while it's
  still running, or waiting to run, is skipped and noted in its history.
- A job is like a chat in the background: it runs alongside the agent's
  chats and its other jobs, sharing the agent's folder, model and daily
  limit. Their model calls simply queue at the model server if it answers
  one at a time (the built-in engine does). Only a test run in the loop
  editor has the agent to itself: jobs and chats wait for it, and it waits
  for them.
- At most *N* runs go at once (Settings → General, 1 by default).
- A scheduled time noticed more than 10 minutes late counts as missed.
- **Pause scheduled jobs** stops scheduled and folder runs; times that pass
  while paused are skipped, not caught up. Runs started by hand still go.
- A job that asks the user something (an *Ask me* block, or a tool call that
  needs an OK) waits, shows the question in the agent's chat and the
  Automation menu, and sends a notification.

### State and history

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

## chummyd

### The bundle (`.chummyd`)

chummyd's whole configuration, made by the app's Configure chummyd window
and read by chummyd at start-up (`crates/bundle`). One file:

```text
"CHUMMYD\x01"         8 bytes
header length         u32, little-endian (at most 4 KiB)
header                JSON: format "chummyd.bundle", version 1, instance_id,
                      kdf { name "argon2id", memory_kib, iterations, lanes, salt }
nonce                 24 bytes
ciphertext            XChaCha20-Poly1305 of the configuration's JSON
```

The key is Argon2id of the password with the header's settings (new bundles:
64 MiB, 3 passes, 1 lane; settings outside 8 MiB–1 GiB, 1–10 passes or 1–8
lanes are refused). The associated data is everything before the nonce, so
the header can't be changed either. The password must pass a strength check
(at least 12 characters and about 64 bits by a rough estimate); the app can
generate one (five groups of five from a 32-character alphabet). Bundles are
at most 64 MiB, and never extracted to disk.

The configuration (`chummyd.config` v2, strict: unknown fields are refused;
v1, without Matrix, still opens):

| Field | |
|---|---|
| `instance` | `id` (random; binds the state folder), `name`, `time_zone` (IANA) |
| `limits` | `max_runs`: job runs at once, 1–8 |
| `permissions`, `tools` | as in the app's settings |
| `providers`, `default_provider` | model connections (never the built-in engine) |
| `secrets` | by name, `provider:<id>` for a connection's key |
| `agents` | `id`, `name`, `loop_id`, `model`, `tools`, `network`, `loop_config`, `model_options`, `channel_approvals` |
| `loops` | the loops, by id: trusted because the bundle authenticates |
| `jobs` | `chummy.job` documents |
| `channels` | `terminal.enabled`; `matrix` (optional, below) |
| `logging` | `message_text` |
| `state_key` | 32 bytes, base64: seals the state folder |

`channels.matrix`: `homeserver` (https, or http only to this computer or
the local network), `user_id` (chummyd's account, `@name:server`),
`password` (to log in the first time and for cross-signing's password
check), `users` (agent id → Matrix IDs allowed to talk to it), `report_to`
(job id → one of its agent's users), `upload_mb` (1–1024, default 100).

`Config::check()` gives errors (chummyd won't start; the app won't export)
and warnings: no channel; a model connection without its key; an agent with
tools that ask first but no approvals over channels (they'd always be
refused); for Matrix, nobody allowed to talk to an agent, or a job that
reports to no one. Errors include an invalid homeserver or Matrix ID, users
for agents not in the bundle, chummyd's own ID among the users, and
`report_to` naming a missing job or someone not allowed for its agent.

### The state folder (`<bundle>.state/`)

```text
lock            held while chummyd runs
state.sealed    "CHUMMYD-STATE\x01", a 24-byte nonce, XChaCha20-Poly1305 of
                the state's JSON with the state key; associated data: the
                magic and the instance id
tools/          the tools' Python and built-in tools (not secret)
matrix/         the Matrix SDK's SQLite stores (end-to-end keys, room
                state), encrypted with the passphrase
                hex(SHA-256("chummyd matrix store" ‖ state key))
```

The state (`chummyd.state` v1, strict): `instance_id`, `saved_at`, `chats`
(conversations: each agent's chat index and messages, at most 500 messages a
conversation and 50 conversations an agent; a Matrix room's conversation is
`m-` and 16 hex digits of SHA-256 of the person's Matrix ID), `paused` (paused runs by
`agent/chat`, base64 of the session's `chummy.paused-run` document), `jobs`
(job state and history), `usage` (the day's model use) and `matrix` (when
the bundle has Matrix: the bot's `user_id`, its `session` (access token and
device), whether it's `cross_signed` and has `synced`, and its `rooms`, one
`{agent, user, room}` each). If the bot's ID changes, the Matrix state and
stores are thrown away and it logs in afresh. A state file that
doesn't open stops chummyd; it's never replaced by a fresh one.
