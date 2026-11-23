"""The Chummy assistant (the default loop): one agent loop, modelled on the
basic behaviour of a coding agent's core loop (Claude Code's query loop),
written as Chummy blocks.

Imported by make_templates.py, which writes assistant.chummy.json.

The shape (root loop):

    Message ─▶ Get ready ─▶ Agent loop ─▶ Reply
                               └ failed ─▶ Reply + Stop with error

Inside the Agent loop (the core cycle):

    request ─▶ Start exchange ─▶ Work (model + tools) ─ tool call ─▶ Show what it says ─▶ Run tool ─▶ Make room ─┐
                                   ▲  ▲                                                                       │
                                   │  └───────────────────────────────────────────────────────────────────────┘
                                   │
                                   └── carry on ◀── Done? ◀── answer (no tool calls)
                                                      └ done ─▶ answer out

What it copies from the original loop, at a basic level:
- One model does the whole job: it is called again and again with the
  growing conversation; every answer that has tool calls gets them run and
  their results added; the first answer without tool calls ends the turn.
- Text the model writes next to its tool calls is shown to you as it goes
  ("I'll read the file first.").
- Planning is the model's own to-do list (like a todo tool): it writes a
  checklist (- [ ] / - [x]) whenever it changes; Chummy keeps the latest one
  and puts it in the system message of every call.
- Tool errors and refused calls go back to the model as the tool's result;
  bad calls are explained and asked again (the engine does this).
- When the conversation reaches the model's compaction point (estimated
  tokens against Settings → Models: 80% of the context the server reports,
  by default), it is summarised and the work carries on from the summary
  (like auto-compaction).
- Project instructions: a CHUMMY.md file at the top of the agent's folder is
  read into the system message (like CLAUDE.md), with a short folder listing.
- Asking you: the model just asks in its answer, and the turn ends; your next
  message carries on.

Added for small models (the original has nothing like it by default, but it
plays the part of its stop checks and to-do reminders): if the model stops
while its to-do list still has unchecked steps, or with an empty answer, it is
told to carry on (twice at most). An answer that ends with a question is never
pushed.
"""

from loopkit import b, l, note, fn

SYSTEM = (
    "You are Chummy, a helpful assistant. You work in the user's folder using the tools you have, and you keep "
    "going until the user's request is done.\n\n"
    "# How you work\n"
    "- Text you write is shown to the user. Before you call tools, say in one short sentence what you're about "
    "to do.\n"
    "- Use tools to look things up and to make changes. Read a file before you change it. Don't guess what you "
    "can check.\n"
    "- When tool calls don't depend on each other, make them together.\n"
    "- If a tool fails or the user doesn't allow a call, read why. Don't repeat the same call: try another way, "
    "or explain what's in the way.\n"
    "- Do what was asked, no more. Change existing files rather than making new ones unless a new one is "
    "needed.\n"
    "- Before anything hard to undo, like deleting or overwriting the user's work, ask the user first.\n"
    "- If the request is unclear, ask one short question and stop there.\n\n"
    "# Planning\n"
    "For work with 3 or more steps, keep a to-do list. Write it as a checklist, one step per line:\n"
    "- [ ] a step to do\n"
    "- [x] a step that's done\n"
    "Write the whole list again whenever it changes: when you make it, and each time you finish a step. Work on "
    "the first unchecked step. Make each step specific. For simple requests, skip the list and just do it.\n\n"
    "# Finishing\n"
    "When everything is done, check your work if you can (for example, read back a file you wrote). Then give "
    "your final answer, with no tool calls: short and direct, what you did and what came out. Be honest: if "
    "something failed or you couldn't check it, say so. Use Markdown when it helps; no emojis unless asked.\n\n"
    "# Your to-do list now\n{{todos}}\n\n"
    "# The user's folder (top level)\n{{folder}}\n\n"
    "# The user's instructions for this folder (CHUMMY.md)\n{{instructions}}\n\n"
    "# Conversation before this request\n{{conversation}}"
)
SUMMARIZE = (
    "Here is your work on the user's request so far:\n\n_INPUT1_\n\n"
    "It's getting long, so write a summary to carry on from. Include what the user asked for, what you've done, "
    "the key facts you found (file names, values, errors, decisions) and what's left to do, with your to-do list "
    "if you have one. Write only the summary, at most 25 short lines."
)
RESUME = (
    "The user's request:\n{{request}}\n\n"
    "Your work on it so far, summarised to save room:\n_INPUT1_\n\n"
    "Carry on from where you left off."
)
OPEN_NUDGE = (
    "The user's request:\n{{request}}\n\n"
    "Your latest work on it:\n{{progress}}\n\n"
    "You then said:\n_INPUT1_\n\n"
    "But your to-do list still has unchecked steps:\n{{todos}}\n\n"
    "If there's more to do, carry on with the next unchecked step now. If every step is actually done or isn't "
    "needed, write the list again with each step checked, then your final answer."
)
EMPTY_NUDGE = (
    "The user's request:\n{{request}}\n\n"
    "Your latest work on it:\n{{progress}}\n\n"
    "Your last reply was empty. If the work is done, give your final answer now. If not, carry on."
)
COMPACTED = "_(The work so far got long, so I summarised it to make room.)_"

# A to-do line: "- [ ] step" or "- [x] step" (also * or +, any indent).
TODO_LINE = r"(?m)^[ \t]*[-*+] \[[ xX]\] .*\S.*$"
# An unchecked to-do line.
OPEN = r"(?m)^[ \t]*[-*+] \[ \]"
HAS_TEXT = r"\S"
ANY = r"(?s)."
# An answer that ends with a question waits for you.
QUESTION = r"\?[\s*_)\"'”»]*$"
# Two nudges given already.
SPENT = r"x{2}"
# The first 60 lines (the folder listing shown to the model).
HEAD = r"\A(?:[^\n]*(?:\n|\z)){1,60}"


def keep_todos_fn():
    return fn(
        "keep_todos", "Keep the to-do list",
        "Finds checklist lines (- [ ] / - [x]) in the model's text and, if there are any, saves them as "
        "{{todos}}, replacing the old list. Text without a checklist leaves the list as it was.",
        [("text", "Text", "text", "in")],
        [("after", "After", "text", "out")],
        [
            b("text", "forward", "Text", 0, 0),
            b("find", "regex", "Checklist lines", 260, 0, {"pattern": TODO_LINE}),
            b("join", "join", "As one list", 520, 0, {"separator": "\n"}),
            b("any", "branch", "Any?", 780, 0, {"pattern": HAS_TEXT},
              "No checklist in this text: keep the old list."),
            b("save", "store", "Save the to-do list", 1040, 0, {"key": "todos", "max_chars": 3000},
              "Read anywhere as {{todos}}."),
        ],
        [
            l("text", "out", "find", "in"), l("find", "out", "join", "in"),
            l("join", "out", "any", "value"), l("join", "out", "any", "test"),
            l("any", "yes", "save", "in", "a new list"),
        ],
        [note("n1", 0, 200, "The model plans with its own checklist, like a to-do tool: it writes the whole list "
                            "whenever it changes, and the latest one is kept.", 320, 110)],
    )


def get_ready_fn():
    return fn(
        "get_ready", "Get ready",
        "Before the first model call: remembers the request and the conversation, reads CHUMMY.md (the user's "
        "instructions for this folder) and the folder's top level, and starts an empty to-do list. Ready fires "
        "once everything is saved.",
        [("message", "Message", "msg", "in"), ("conversation", "Conversation", "conv", "in")],
        [("ready", "Ready", "msg", "out")],
        [
            b("msg", "forward", "Message", 0, 0),
            b("conv", "store", "Remember the conversation", 0, 200, {"key": "conversation", "max_chars": 6000},
              "{{conversation}}: the chat before this message."),
            b("req", "store", "Remember the request", 260, -200, {"key": "request", "max_chars": 8000},
              "{{request}}"),
            # CHUMMY.md
            b("which", "combine", "CHUMMY.md", 260, 0, {"inputs": 1, "format": "CHUMMY.md"}),
            b("read", "read_file", "Read CHUMMY.md", 520, 0, {}, "Only inside the agent's folder."),
            b("no_file", "combine", "No CHUMMY.md", 780, 120, {
                "inputs": 1, "format": "(none: this folder has no CHUMMY.md)"}),
            b("instr", "store", "Remember the instructions", 1040, 0, {"key": "instructions", "max_chars": 4000},
              "{{instructions}}"),
            # The folder
            b("top", "combine", "Top of the folder", 260, 300, {"inputs": 1, "format": "."}),
            b("ls", "list_files", "List the folder", 520, 300),
            b("lines", "join", "One per line", 780, 300, {"separator": "\n"}),
            b("head", "regex", "First 60", 1040, 300, {"pattern": HEAD}),
            b("head_text", "join", "As text", 1300, 300, {"separator": ""}),
            b("not_empty", "branch", "Anything?", 1560, 300, {"pattern": HAS_TEXT}),
            b("empty", "combine", "Empty folder", 1820, 420, {"inputs": 1, "format": "(empty)"}),
            b("no_list", "combine", "Couldn't list", 780, 450, {
                "inputs": 1, "format": "(couldn't list it: _INPUT1_)"}),
            b("folder", "store", "Remember the folder", 2080, 300, {"key": "folder", "max_chars": 3000},
              "{{folder}}"),
            # Starting values
            b("no_todos", "combine", "No to-do list yet", 260, 600, {"inputs": 1, "format": "(none yet)"}),
            b("todos", "store", "Start the to-do list", 520, 600, {"key": "todos", "max_chars": 3000}),
            b("no_progress", "combine", "No work yet", 260, 750, {"inputs": 1, "format": "(nothing yet)"}),
            b("progress", "store", "Start the progress", 520, 750, {"key": "progress", "max_chars": 8000}),
        ],
        [
            l("msg", "out", "req", "in"),
            l("msg", "out", "which", "in1"), l("which", "out", "read", "path"),
            l("read", "out", "instr", "in", "found"), l("read", "error", "no_file", "in1", "no file"),
            l("no_file", "out", "instr", "in"),
            l("msg", "out", "top", "in1"), l("top", "out", "ls", "path"),
            l("ls", "out", "lines", "in"), l("lines", "out", "head", "in"), l("head", "out", "head_text", "in"),
            l("head_text", "out", "not_empty", "value"), l("head_text", "out", "not_empty", "test"),
            l("not_empty", "yes", "folder", "in"), l("not_empty", "no", "empty", "in1"),
            l("empty", "out", "folder", "in"),
            l("ls", "error", "no_list", "in1"), l("no_list", "out", "folder", "in"),
            l("msg", "out", "no_todos", "in1"), l("no_todos", "out", "todos", "in"),
            l("msg", "out", "no_progress", "in1"), l("no_progress", "out", "progress", "in"),
        ],
        [note("n1", 0, 900, "Like a coding agent's start-up context: the project's instruction file (here "
                            "CHUMMY.md) and a look at the folder. Everything stays inside the agent's folder.",
              420, 100)],
    )


def show_fn():
    return fn(
        "show", "Show what it says",
        "When the model calls tools, whatever it wrote alongside the calls (\"I'll read the file first.\") is "
        "shown to you, and any checklist in it becomes the to-do list. The exchange goes on unchanged.",
        [("exchange", "Tool call", "ex", "in")],
        [("exchange", "Tool call", "ex", "out")],
        [
            b("ex", "forward", "Tool call", 0, 0),
            b("read", "read_exchange", "What it said", 260, 0, {"role": "assistant"},
              "Newest message: empty when the model only called tools, so older text is never shown twice."),
            b("said", "branch", "Said anything?", 520, 0, {"pattern": HAS_TEXT}),
            b("reply", "output", "Show it", 780, -100),
            b("todos", "function", "Keep the to-do list", 780, 100, {"function": "keep_todos"}),
        ],
        [
            l("ex", "out", "read", "in"),
            l("read", "newest", "said", "value"), l("read", "newest", "said", "test"),
            l("said", "yes", "reply", "in"), l("said", "yes", "todos", "text"),
        ],
    )


def make_room_fn():
    return fn(
        "make_room", "Make room",
        "After each round of tool results: saves the latest work as {{progress}}, and if the conversation has "
        "reached the model's compaction point (Settings → Models: 80% of its context unless you change it), the "
        "model summarises it and the work carries on from the summary in a fresh exchange. If summarising "
        "fails, the conversation goes on as it was.",
        [("exchange", "Results", "ex", "in")],
        [("exchange", "Exchange", "out", "out")],
        [
            b("ex", "forward", "Results", 0, 0),
            b("read", "read_exchange", "The work so far", 260, -200,
              {"role": "any", "system_in_transcript": False}),
            b("save", "store", "Save the progress", 520, -200, {"key": "progress", "max_chars": 8000},
              "{{progress}} keeps the newest 8,000 characters."),
            b("check", "context_check", "Context full?", 260, 100, {},
              "Estimated tokens (system message included) against the model's compaction point."),
            b("read_full", "read_exchange", "Everything so far", 560, -50,
              {"role": "any", "system_in_transcript": False}),
            b("ask", "combine", "Ask for a summary", 820, -50, {"inputs": 1, "format": SUMMARIZE}),
            b("summ", "llm", "Summarise the work", 1080, -50, {"temperature": 0.2}),
            b("resume", "combine", "Carry on from it", 1340, -50, {"inputs": 1, "format": RESUME}),
            b("restart", "exchange", "Fresh exchange", 1600, -50, {"system": SYSTEM, "roles": ["user"]},
              "The same instructions, then the summary."),
            b("say", "combine", "Tell you", 1340, -250, {"inputs": 1, "format": COMPACTED}),
            b("show", "output", "Show that", 1600, -250),
            b("keep", "branch", "If that fails", 1340, 200, {"pattern": ANY},
              "Carry on with the long conversation."),
            b("out", "forward", "Exchange", 1860, 100),
        ],
        [
            l("ex", "out", "read", "in"), l("read", "all", "save", "in"),
            l("ex", "out", "check", "in"),
            l("check", "room", "out", "in", "has room"),
            l("check", "full", "read_full", "in", "full"), l("read_full", "all", "ask", "in1"),
            l("ask", "out", "summ", "in"),
            l("summ", "out", "resume", "in1"), l("resume", "out", "restart", "in"),
            l("restart", "out", "out", "in", "from the summary"),
            l("summ", "out", "say", "in1"), l("say", "out", "show", "in"),
            l("check", "full", "keep", "value"), l("summ", "error", "keep", "test"),
            l("keep", "yes", "out", "in", "summary failed"),
        ],
        [note("n1", 0, 350, "Like auto-compaction: a long conversation is summarised so the model's context "
                            "doesn't overflow. When it happens is set per model in Settings → Models (Make room "
                            "at), or here with Context full?'s own percentage.", 420, 130)],
    )


def done_fn():
    return fn(
        "done_check", "Done?",
        "When the model answers without tool calls. Keeps any checklist in the answer, then decides: an answer "
        "that ends with a question waits for you; an empty answer, or one given while the to-do list still has "
        "unchecked steps, is sent back to carry on (twice at most per message); anything else is the final "
        "answer.",
        [("answer", "Answer", "ans", "in")],
        [("done", "Final answer", "done", "out"), ("again", "Carry on", "again", "out")],
        [
            b("ans", "forward", "Answer", 0, 0),
            b("todos_fn", "function", "Keep the to-do list", 260, 0, {"function": "keep_todos"}),
            b("nudged", "recall", "Times sent back", 520, -200, {"key": "nudged"}),
            b("spent", "branch", "Sent back twice?", 780, 0, {"pattern": SPENT},
              "Twice already: take this answer."),
            b("question", "branch", "Asks you?", 1040, 0, {"pattern": QUESTION},
              "Ends with a question: the turn ends and your reply carries on."),
            b("empty", "branch", "Any answer?", 1300, 0, {"pattern": HAS_TEXT}),
            b("todos", "recall", "The to-do list", 1560, -200, {"key": "todos"}),
            b("open", "branch", "Unchecked steps?", 1820, 0, {"pattern": OPEN}),
            b("open_msg", "combine", "Carry on with the list", 2080, 150, {"inputs": 1, "format": OPEN_NUDGE}),
            b("show", "output", "Show what it said", 2080, -250, {}, "You still see what the model said."),
            b("empty_msg", "combine", "Carry on after nothing", 1560, 300, {"inputs": 1, "format": EMPTY_NUDGE}),
            b("again", "forward", "Carry on", 2340, 250),
            b("count", "combine", "Count it", 2600, 400, {"inputs": 1, "format": "{{nudged}}x"}),
            b("save_count", "store", "Remember the count", 2860, 400, {"key": "nudged", "max_chars": 100}),
            b("done", "forward", "Final answer", 2340, -100),
        ],
        [
            l("ans", "out", "todos_fn", "text"),
            l("todos_fn", "after", "nudged", "in"),
            l("todos_fn", "after", "spent", "value"), l("nudged", "out", "spent", "test"),
            l("spent", "yes", "done", "in", "sent back twice"),
            l("spent", "no", "question", "value"), l("spent", "no", "question", "test"),
            l("question", "yes", "done", "in", "a question for you"),
            l("question", "no", "empty", "value"), l("question", "no", "empty", "test"),
            l("empty", "no", "empty_msg", "in1", "empty"), l("empty_msg", "out", "again", "in"),
            l("empty", "yes", "todos", "in"),
            l("empty", "yes", "open", "value"), l("todos", "out", "open", "test"),
            l("open", "yes", "open_msg", "in1", "steps left"), l("open", "yes", "show", "in"), l("open_msg", "out", "again", "in"),
            l("open", "no", "done", "in", "finished"),
            l("again", "out", "count", "in1"), l("count", "out", "save_count", "in"),
        ],
        [note("n1", 0, 300, "Not in the original loop, which simply stops when the model stops calling tools. "
                            "Small models often stop half-way, so this sends them back (at most twice). Delete "
                            "the Done? block in the Agent loop and link Answer straight out to get the original "
                            "behaviour.", 420, 130)],
    )


def agent_fn():
    return fn(
        "agent", "Agent loop",
        "The core loop: the model is called with the conversation so far; if it calls tools, what it said is "
        "shown, the calls run (your approvals apply) and the results go back to it; when it answers without "
        "tool calls, Done? decides whether that's the final answer.",
        [("request", "Request", "req", "in")],
        [("answer", "Final answer", "answer", "out"), ("failed", "Failed", "failed", "out")],
        [
            b("req", "forward", "Request", 0, 0),
            b("start", "exchange", "Start the exchange", 260, 0, {"system": SYSTEM, "roles": ["user"]},
              "The instructions (filled in fresh at every call), then your message."),
            b("work", "llm", "Work", 560, 0, {"retries": 3, "on_limit": "ask"},
              "Uses the tools this agent allows. The system message (with {{todos}}) is filled in fresh at "
              "every call."),
            b("show", "function", "Show what it says", 860, 250, {"function": "show"}),
            b("run", "run_tool", "Run tool", 1160, 250, {"max_result_chars": 4000},
              "Errors and calls you don't allow go back to the model as the result."),
            b("room", "function", "Make room", 1460, 250, {"function": "make_room"},
              "Summarises a long conversation."),
            b("check", "function", "Done?", 860, -200, {"function": "done_check"}),
            b("restart", "exchange", "Carry on", 1160, -350, {"system": SYSTEM, "roles": ["user"]}),
            b("answer", "forward", "Final answer", 1160, -150),
            b("failed", "forward", "Failed", 860, 500),
        ],
        [
            l("req", "out", "start", "in"), l("start", "out", "work", "in"),
            l("work", "tool", "show", "exchange", "tool calls"), l("show", "exchange", "run", "in"),
            l("run", "out", "room", "exchange", "results"), l("room", "exchange", "work", "in", "next turn"),
            l("work", "out", "check", "answer", "no tool calls"),
            l("check", "done", "answer", "in"),
            l("check", "again", "restart", "in", "not done yet"), l("restart", "out", "work", "in"),
            l("work", "error", "failed", "in", "the model call failed"),
        ],
        [note("n1", 0, -500, "The core cycle: Work ⇄ Show what it says ⇄ Run tool ⇄ Make room, until the model "
                             "answers without tool calls. The loop's limits (model calls, tool calls) stop a "
                             "model that never finishes.", 460, 100)],
    )


def assistant():
    blocks = [
        b("message", "input", "Your message", 0, 0),
        b("history", "history", "Recent conversation", 0, 200, {"max_messages": 20}),
        b("ready", "function", "Get ready", 300, 100, {"function": "get_ready"},
          "Reads CHUMMY.md and the folder, starts the to-do list."),
        b("agent", "function", "Agent loop", 600, 100, {"function": "agent"},
          "The model works with tools until it's done."),
        b("reply", "output", "Reply", 900, 0),
        b("why", "combine", "What went wrong", 900, 250, {
            "inputs": 1, "format": "**I couldn't finish:** _INPUT1_"}),
        b("show_why", "output", "Show it", 1200, 200),
        b("stop", "stop", "Stop with error", 1200, 350, {}, "The run ends as failed, with the reason."),
    ]
    links = [
        l("message", "out", "ready", "message"), l("history", "out", "ready", "conversation"),
        l("ready", "ready", "agent", "request"),
        l("agent", "answer", "reply", "in", "final answer"),
        l("agent", "failed", "why", "in1", "the model call failed"),
        l("why", "out", "show_why", "in"), l("why", "out", "stop", "in"),
    ]
    return {
        "format": "chummy.loop", "version": 3, "name": "Chummy assistant",
        "description": (
            "The default loop. One agent loop, like a coding agent's: the model works with tools, call after call, "
            "until it "
            "answers without one. It says what it's doing as it goes, plans with its own to-do list when the work "
            "has several steps, reads CHUMMY.md from the folder for your instructions, and summarises the "
            "conversation when it gets long. If it stops with steps unchecked, it's sent back to finish (twice "
            "at most)."
        ),
        "limits": {"max_steps": 10000, "max_llm_calls": 200, "max_tool_calls": 150},
        "blocks": blocks, "links": links,
        "functions": [keep_todos_fn(), get_ready_fn(), show_fn(), make_room_fn(), done_fn(),
                      agent_fn()],
        "notes": [
            note("n1", 0, 450,
                 "Chummy assistant\n\n"
                 "1. Get ready: your request, the conversation, CHUMMY.md (your instructions for this folder) "
                 "and the folder's top level.\n"
                 "2. Agent loop: one model does the work. Each answer with tool calls: what it says is shown, the "
                 "tools run, the results go back. Long conversations are summarised.\n"
                 "3. Planning: the model keeps its own checklist (- [ ] / - [x]) as {{todos}}.\n"
                 "4. Done? An answer without tool calls is the final answer, unless it's empty or steps are "
                 "unchecked (then it's sent back, twice at most). A question for you ends the turn.\n\n"
                 "Double-click a function block to see inside.", 460, 300),
        ],
    }
