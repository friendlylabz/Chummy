"""Writes the planner template loop files.

The templates are ordinary chummy.loop files; this script only keeps the two
planner variants consistent. Run it after changing them:

    python3 crates/flow/templates/make_templates.py

Shape of both planners (the root loop):

    Goal ──▶ Tasks to do ──▶ Next task ──▶ Attempt ──▶ message ─▶ Reply
                ▲               ▲  ▲          │ done ──────┘ (back to Next task)
                │               │  │          └ breakdown ─▶ Plan ─▶ message ─▶ Reply
                │               │  └──────── after ─────────┘  │
                └──── Up to 5 ◀──────────── steps ─────────────┘

A running exchange (the goal, then each task and its result) waits at
"Next task" and pairs with one task at a time, so tasks go strictly in
order. The goal itself is the first task: it is attempted before anything is
planned. A task the model says is TOO BIG is broken into 5 or fewer smaller
steps, which join the back of the queue, so the work goes breadth-first.

The verify variant adds a Verify function (the model writes a yes/no check
question for an answer, then asks itself that question). An attempt that
fails its check is broken down; a plan that fails its check is made again
once, with the reason.
"""

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

TOOLS = ["read_file", "write_file", "list_files", "search_files", "fetch_page", "calculator", "current_time"]
STEP = r"(?m)^\s*\d+[.)]\s*(.+?)\s*$"
TOO_BIG = r"(?i)^\W*too\s*big"
YES = r"(?i)^\W*yes"

SYSTEM = "You are working toward the user's goal one task at a time. Keep every answer short and concrete."
ATTEMPT = (
    "Do the user's latest task now. Use a tool if it helps. If you can do it, give the result in a few "
    "sentences. If it is too big to finish in one go, reply with only: TOO BIG"
)
BREAK = (
    "_INPUT_\n\nThis is too big to do in one go. Break it into 5 or fewer smaller steps, as broad or as narrow "
    "as each needs to be. Write only the steps, one per line, numbered like:\n1. First step\n2. Second step"
)
QUESTION = (
    "Request: _INPUT1_\n\nAnswer: _INPUT2_\n\nWrite one yes-or-no question that checks this answer against the "
    "request, such as \"Does this answer make sense given the request?\" Write only the question."
)
CHECK = (
    "Request: _INPUT1_\n\nAnswer: _INPUT2_\n\nQuestion: _INPUT3_\n\nAnswer the question about the answer above. "
    "Start with YES or NO, then give one short reason."
)


def b(id, type, label, x, y, params=None, comment=""):
    out = {"id": id, "type": type, "label": label, "position": {"x": x, "y": y}, "params": params or {}}
    if comment:
        out["comment"] = comment
    return out


def l(a, ap, c, cp, label=""):
    out = {"from": {"block": a, "port": ap}, "to": {"block": c, "port": cp}}
    if label:
        out["label"] = label
    return out


def note(id, x, y, text, w=320, h=0):
    return {"id": id, "text": text, "position": {"x": x, "y": y}, "width": w, "height": h}


def fn(id, name, description, inputs, outputs, blocks, links, notes=None):
    out = {
        "id": id,
        "name": name,
        "description": description,
        "inputs": [{"name": n, "label": lab, "to": {"block": blk, "port": p}} for n, lab, blk, p in inputs],
        "outputs": [{"name": n, "label": lab, "from": {"block": blk, "port": p}} for n, lab, blk, p in outputs],
        "blocks": blocks,
        "links": links,
    }
    if notes:
        out["notes"] = notes
    return out


def verify_fn():
    return fn(
        "verify",
        "Verify",
        "Writes a yes/no check question for an answer, then asks the model that question about it. "
        "Passed sends the answer on; Failed sends the reason.",
        [("request", "Request", "req", "in"), ("answer", "Answer", "ans", "in")],
        [
            ("passed", "Passed", "pass", "yes"),
            ("failed", "Failed (reason)", "why", "no"),
            ("verdict", "Verdict", "check", "out"),
        ],
        [
            b("req", "forward", "Request", 0, 0),
            b("ans", "forward", "Answer", 0, 200),
            b("ask_q", "combine", "Ask for a check", 260, 0, {"inputs": 2, "format": QUESTION}),
            b("question", "llm", "Write a check question", 520, 0, {
                "system": "Write one short yes-or-no question.", "temperature": 0.2, "max_tokens": 80}),
            b("ask_c", "combine", "Ask the check", 780, 100, {"inputs": 3, "format": CHECK}),
            b("check", "llm", "Answer the check", 1040, 100, {
                "system": "Answer YES or NO first, then give one short reason.", "temperature": 0.0,
                "max_tokens": 80}),
            b("pass", "branch", "Passed?", 1300, 200, {"pattern": YES}),
            b("why", "branch", "Reason if not", 1300, 0, {"pattern": YES}),
        ],
        [
            l("req", "out", "ask_q", "in1"), l("ans", "out", "ask_q", "in2"),
            l("ask_q", "out", "question", "in"),
            l("req", "out", "ask_c", "in1"), l("ans", "out", "ask_c", "in2"), l("question", "out", "ask_c", "in3"),
            l("ask_c", "out", "check", "in"),
            l("ans", "out", "pass", "value"), l("check", "out", "pass", "test"),
            l("check", "out", "why", "value"), l("check", "out", "why", "test"),
        ],
    )


def attempt_fn(verify):
    blocks = [
        b("entry", "forward", "Exchange", 0, 0),
        b("work", "llm", "Try the task", 260, 0, {
            "system": ATTEMPT, "max_tokens": 800, "tools": TOOLS, "retries": 2}),
        b("run", "run_tool", "Run tool", 520, 200, {"max_result_chars": 4000}),
        b("which", "read_exchange", "Which task", 260, -200, {"role": "user"}),
        b("big_answer", "branch", "Too big? (answer)", 780, 0, {"pattern": TOO_BIG}),
        b("big_exchange", "branch", "Too big? (exchange)", 780, 200, {"pattern": TOO_BIG}),
        b("add_result", "augment", "Add result", 1300, 100, {"role": "assistant", "format": "_INPUT_"}),
        b("message", "combine", "Result message", 1300, -150, {"inputs": 2, "format": "**✓ _INPUT1_**\n\n_INPUT2_"}),
        b("to_plan", "forward", "Needs a plan", 1300, 350),
    ]
    links = [
        l("entry", "out", "work", "in"), l("work", "tool", "run", "in"), l("run", "out", "work", "in"),
        l("entry", "out", "which", "in"),
        l("work", "out", "big_answer", "value"), l("work", "out", "big_answer", "test"),
        l("entry", "out", "big_exchange", "value"), l("work", "out", "big_exchange", "test"),
        l("big_exchange", "yes", "to_plan", "in"),
        l("which", "last", "message", "in1"),
    ]
    if not verify:
        links += [
            l("big_answer", "no", "add_result", "in"), l("big_exchange", "no", "add_result", "exchange"),
            l("big_answer", "no", "message", "in2"),
        ]
        desc = ("The model tries the task, calling tools as often as it needs. If it says the task is too big, "
                "the exchange goes out of Breakdown instead.")
    else:
        blocks += [
            b("verify", "function", "Verify the result", 1040, -50, {"function": "verify"}),
            b("checked", "branch", "Passed the check?", 1040, 200, {"pattern": YES}),
            b("note", "augment", "Note the failed check", 1300, 500, {
                "role": "assistant", "format": "My attempt didn't pass this check: _INPUT_"}),
        ]
        links += [
            l("which", "last", "verify", "request"), l("big_answer", "no", "verify", "answer"),
            l("big_exchange", "no", "checked", "value"), l("verify", "verdict", "checked", "test"),
            l("checked", "yes", "add_result", "exchange"), l("verify", "passed", "add_result", "in"),
            l("verify", "passed", "message", "in2"),
            l("checked", "no", "note", "exchange"), l("verify", "failed", "note", "in"),
            l("note", "out", "to_plan", "in"),
        ]
        desc = ("The model tries the task, calling tools as often as it needs, and checks its result with "
                "Verify. If it says the task is too big, or the result fails its check, the exchange goes out "
                "of Breakdown instead.")
    return fn(
        "attempt", "Attempt the task", desc,
        [("exchange", "Exchange", "entry", "in")],
        [
            ("message", "Result message", "message", "out"),
            ("done", "Exchange with result", "add_result", "out"),
            ("breakdown", "Breakdown", "to_plan", "out"),
        ],
        blocks, links,
    )


def plan_fn(verify):
    blocks = [
        b("entry", "forward", "Exchange", 0, 0),
        b("which", "read_exchange", "Which task", 260, 200, {"role": "user"}),
        b("ask", "augment", "Ask for smaller steps", 520, 0, {"role": "user", "format": BREAK}),
        b("sub", "llm", "Plan smaller steps", 780, 0, {"temperature": 0.3, "max_tokens": 400}),
        b("plan", "forward", "The plan", 1300, 0),
        b("parse", "capture", "Steps", 1560, 0, {"pattern": STEP}),
        b("message", "combine", "Plan message", 1560, 200, {
            "inputs": 2, "format": "**Breaking down _INPUT1_**\n\n_INPUT2_"}),
        b("after", "augment", "Add the plan", 1560, -200, {
            "role": "assistant", "format": "I broke this task into smaller steps:\n_INPUT_"}),
    ]
    links = [
        l("entry", "out", "ask", "exchange"), l("entry", "out", "which", "in"),
        l("which", "last", "ask", "in"), l("ask", "out", "sub", "in"),
        l("plan", "out", "parse", "in"), l("plan", "out", "message", "in2"), l("plan", "out", "after", "in"),
        l("which", "last", "message", "in1"), l("entry", "out", "after", "exchange"),
    ]
    if not verify:
        links += [l("sub", "out", "plan", "in")]
        desc = "Plans 5 or fewer smaller steps for a task that's too big."
    else:
        blocks += [
            b("verify", "function", "Verify the plan", 1040, 0, {"function": "verify"}),
            b("with_plan", "augment", "Keep the first plan", 1040, 250, {"role": "assistant", "format": "_INPUT_"}),
            b("checked", "branch", "Passed the check?", 1300, 250, {"pattern": YES}),
            b("feedback", "augment", "Explain the failed check", 1300, 450, {
                "role": "user",
                "format": "That plan didn't pass this check: _INPUT_\n\nWrite a better plan in the same format."}),
            b("again", "llm", "Plan again", 1040, 450, {"temperature": 0.3, "max_tokens": 400}),
        ]
        links += [
            l("which", "last", "verify", "request"), l("sub", "out", "verify", "answer"),
            l("verify", "passed", "plan", "in"),
            l("ask", "out", "with_plan", "exchange"), l("sub", "out", "with_plan", "in"),
            l("with_plan", "out", "checked", "value"), l("verify", "verdict", "checked", "test"),
            l("checked", "no", "feedback", "exchange"), l("verify", "failed", "feedback", "in"),
            l("feedback", "out", "again", "in"), l("again", "out", "plan", "in"),
        ]
        desc = ("Plans 5 or fewer smaller steps for a task that's too big, checks the plan with Verify, and "
                "plans once more, with the reason, if it fails.")
    return fn(
        "break_down", "Plan smaller steps", desc,
        [("exchange", "Exchange", "entry", "in")],
        [
            ("message", "Plan message", "message", "out"),
            ("after", "Exchange with plan", "after", "out"),
            ("steps", "Steps", "parse", "group1"),
        ],
        blocks, links,
    )


def planner(verify):
    blocks = [
        b("goal", "input", "Goal", 0, 0),
        b("goal_text", "combine", "Goal text", 260, -180, {"inputs": 1, "format": "Goal: _INPUT1_"}),
        b("start", "exchange", "Running exchange", 520, -180, {"system": SYSTEM, "roles": ["user"]}),
        b("queue", "forward", "Tasks to do", 520, 80),
        b("pair", "augment", "Next task", 780, 0, {"role": "user", "format": "Task: _INPUT_"}),
        b("attempt", "function", "Attempt the task", 1040, 0, {"function": "attempt"}),
        b("show_result", "output", "Show result", 1300, -180),
        b("plan", "function", "Plan smaller steps", 1300, 120, {"function": "break_down"}),
        b("show_plan", "output", "Show plan", 1560, 120),
        b("more", "for_each", "Up to 5 steps", 1560, 320, {"limit": 5}),
    ]
    links = [
        l("goal", "out", "goal_text", "in1"), l("goal", "out", "queue", "in"),
        l("goal_text", "out", "start", "in"), l("start", "out", "pair", "exchange"),
        l("queue", "out", "pair", "in"), l("pair", "out", "attempt", "exchange"),
        l("attempt", "message", "show_result", "in"), l("attempt", "done", "pair", "exchange"),
        l("attempt", "breakdown", "plan", "exchange"),
        l("plan", "message", "show_plan", "in"), l("plan", "after", "pair", "exchange"),
        l("plan", "steps", "more", "in"), l("more", "out", "queue", "in"),
    ]
    functions = [attempt_fn(verify), plan_fn(verify)]
    if verify:
        functions.insert(0, verify_fn())
    name = "5 step recursive plan verify" if verify else "5 step recursive plan"
    description = (
        "Treats your message as a goal and tries to do it straight away, by writing or with tools. Anything "
        "too big is broken into 5 or fewer smaller steps, and each step is tried the same way, breadth-first. "
        "Every result and plan is shown, and added to one running exchange."
    )
    if verify:
        description += (
            " Each result and plan is also checked: the model writes a yes/no question for it and answers it. "
            "Results that fail are broken down; plans that fail are made again once. Slower, but more careful."
        )
    limits = {"max_steps": 1500, "max_llm_calls": 120, "max_tool_calls": 40} if verify else \
        {"max_steps": 800, "max_llm_calls": 60, "max_tool_calls": 30}
    return {
        "format": "chummy.loop", "version": 2, "name": name, "description": description, "limits": limits,
        "blocks": blocks, "links": links, "functions": functions,
    }



# ---------------------------------------------------------------------------
# The Chummy assistant: the default loop, picked for you in onboarding.
#
#   Your message ─▶ Clarify ─▶ Tasks to do ─▶ Next task ─▶ Attempt ─▶ Show result
#        │            (asks you                   ▲  ▲        │  └▶ Update notes
#        │             if unclear)                │  │        └ breakdown ─▶ Plan ─▶ Show plan
#        └▶ Conversation ─▶ Running exchange ─────┘  └── after ───────────┘   └▶ Update notes
#                                                          steps ─▶ Up to 5 ─▶ Tasks to do
#
# On top of the verify planner it:
# - asks you one question first if your request is unclear, and again
#   whenever a task can't go on without you (QUESTION: …);
# - checks every tool call with the model before it runs (declined calls go
#   back to the model with the reason);
# - keeps running working notes in memory ({{notes}}), shown to the model in
#   every attempt and plan, and rewritten by the model after each step;
# - catches failures (Failed outputs) and carries on with a message instead
#   of stopping.

A_SYSTEM = (
    "You are a careful assistant working toward the user's goal one task at a time. "
    "Keep every answer short and concrete.\n\nYour working notes:\n{{notes}}"
)
A_ATTEMPT = (
    "Do the user's latest task now. Use a tool if it helps. If you can do it, give the result in a few "
    "sentences. If it is too big to finish in one go, reply with only: TOO BIG\\n"
    "If you can't go on without the user's input, reply with only: QUESTION: and your question.\n\n"
    "Your working notes:\n{{notes}}"
).replace("\\n", "\n")
CLARIFY = (
    "_INPUT_\n\nIs the user's latest request clear enough to act on? If it is, reply with only: CLEAR\n"
    "If not, reply with the one short question you most need answered."
)
CLARIFIED = "_INPUT1_\n\n(I asked: _INPUT2_\nThe user answered: _INPUT3_)"
CHECK_CALL = (
    "Task: _INPUT1_\n\nThe next step calls a tool:\n_INPUT2_\n\nIs this call sensible and safe for the task? "
    "Start with YES or NO, then give one short reason."
)
NOTES = (
    "Your working notes so far:\n{{notes}}\n\nWhat just happened:\n_INPUT_\n\nRewrite the working notes for the "
    "rest of the work: key facts found, decisions made, what is done and what is left. At most 10 short lines. "
    "Write only the notes."
)
QUESTION = r"(?i)^\W*question\s*:"
ANY = r"(?s)."
NO_ANSWER = "No answer came. Use your best judgement and carry on."


def a_verify_fn():
    f = verify_fn()
    f["description"] += " If a check can't run, the answer passes, so a failing model never blocks the work."
    f["blocks"] += [
        b("q_fallback", "combine", "Fallback question", 520, 250,
          {"inputs": 1, "format": "Does this answer make sense given the request?"},
          "If writing a question fails, use the standard one."),
        b("c_fallback", "combine", "Pass if the check fails", 1040, 300,
          {"inputs": 1, "format": "YES (the check couldn't run: _INPUT1_)"},
          "A check that can't run doesn't hold the work up."),
        b("verdict_fwd", "forward", "Verdict", 1560, 100),
    ]
    f["links"] = [x for x in f["links"]] + [
        l("question", "error", "q_fallback", "in1"), l("q_fallback", "out", "ask_c", "in3"),
        l("check", "error", "c_fallback", "in1"),
        l("c_fallback", "out", "pass", "test"), l("c_fallback", "out", "why", "value"),
        l("c_fallback", "out", "why", "test"),
        l("check", "out", "verdict_fwd", "in"), l("c_fallback", "out", "verdict_fwd", "in"),
    ]
    f["outputs"] = [o for o in f["outputs"] if o["name"] != "verdict"] + [
        {"name": "verdict", "label": "Verdict", "from": {"block": "verdict_fwd", "port": "out"}}]
    for blk in f["blocks"]:
        if blk["id"] == "question":
            blk["comment"] = "A small model checks itself best with a question it wrote."
        if blk["id"] == "check":
            blk["comment"] = "Must start with YES or NO."
    f["notes"] = [note("n1", 0, 380, "Verify: the model writes a yes/no question about an answer, then answers "
                       "it. Passed sends the answer on; Failed sends the reason.")]
    return f


def clarify_fn():
    return fn(
        "clarify", "Clarify the request",
        "Asks the model whether the request is clear enough. If not, asks you its one question, and adds your "
        "answer to the request. Any failure lets the request through unchanged.",
        [("context", "Conversation and request", "judge", "in"), ("request", "Request", "req", "in")],
        [("request", "Request", "out", "out")],
        [
            b("req", "forward", "Request", 0, 250),
            b("judge", "llm", "Clear enough?", 0, 0, {
                "prompt": CLARIFY, "system": "Reply with CLEAR or one short question.", "temperature": 0.0,
                "max_tokens": 100}, "Replies CLEAR, or with one question."),
            b("clear", "branch", "Clear?", 300, 100, {"pattern": r"(?i)^\W*clear"}),
            b("unclear", "branch", "Question", 300, -100, {"pattern": r"(?i)^\W*clear"}),
            b("ask", "ask_user", "Ask you", 600, -100, {
                "prompt": "_INPUT_", "timeout_minutes": 60, "default_answer": NO_ANSWER},
                "Waits up to an hour, then carries on without an answer."),
            b("added", "combine", "Add your answer", 900, -50, {"inputs": 3, "format": CLARIFIED}),
            b("no_answer", "combine", "No answer", 600, 100, {"inputs": 1, "format": NO_ANSWER}),
            b("as_is", "branch", "Use as is", 600, 300, {"pattern": ANY},
              "If the check fails, the request goes on unchanged."),
            b("out", "forward", "Request", 1200, 100),
        ],
        [
            l("req", "out", "clear", "value"), l("judge", "out", "clear", "test"),
            l("clear", "yes", "out", "in", "clear: as it is"),
            l("judge", "out", "unclear", "value"), l("judge", "out", "unclear", "test"),
            l("unclear", "no", "ask", "in", "the question"),
            l("req", "out", "added", "in1"), l("unclear", "no", "added", "in2"), l("ask", "out", "added", "in3"),
            l("ask", "error", "no_answer", "in1"), l("no_answer", "out", "added", "in3"),
            l("added", "out", "out", "in"),
            l("req", "out", "as_is", "value"), l("judge", "error", "as_is", "test"),
            l("as_is", "yes", "out", "in", "the check failed"),
        ],
        [note("n1", 0, 450, "Clarify runs once, before any work. Clear requests pass straight through; "
                            "unclear ones get one question to you.")],
    )


def check_call_fn():
    return fn(
        "check_call", "Check the tool call",
        "Before a tool call runs, the model checks that it makes sense and is safe for the task. Approved calls go "
        "on to Run tool; others are declined with the reason, which goes back to the model. If the check can't "
        "run, the call is approved (your own approvals still apply).",
        [("exchange", "Tool call", "ex", "in")],
        [("approved", "Approved", "ok", "out"), ("declined", "Declined", "decline", "out")],
        [
            b("ex", "forward", "Tool call", 0, 100),
            b("read", "read_exchange", "Task and call", 250, 0, {"role": "user"}),
            b("ask", "combine", "Ask about the call", 500, 0, {"inputs": 2, "format": CHECK_CALL}),
            b("judge", "llm", "Sensible and safe?", 750, 0, {
                "system": "Answer YES or NO first, then give one short reason.", "temperature": 0.0,
                "max_tokens": 80}, "Must start with YES or NO."),
            b("br", "branch", "Approved?", 1000, 150, {"pattern": YES}),
            b("why", "branch", "Reason if not", 1000, -50, {"pattern": YES}),
            b("decline", "decline_calls", "Decline the call", 1250, 50, {},
              "Answers the call with “Not run: <reason>”, so the model can try something else."),
            b("err", "branch", "Check failed", 1000, 350, {"pattern": ANY}),
            b("ok", "forward", "Approved", 1250, 250),
        ],
        [
            l("ex", "out", "read", "in"), l("read", "last", "ask", "in1"), l("read", "calls", "ask", "in2"),
            l("ask", "out", "judge", "in"),
            l("ex", "out", "br", "value"), l("judge", "out", "br", "test"),
            l("judge", "out", "why", "value"), l("judge", "out", "why", "test"),
            l("br", "yes", "ok", "in", "yes"), l("br", "no", "decline", "in", "no"),
            l("why", "no", "decline", "reason", "the reason"),
            l("ex", "out", "err", "value"), l("judge", "error", "err", "test"),
            l("err", "yes", "ok", "in", "check couldn't run"),
        ],
    )


def update_notes_fn():
    return fn(
        "update_notes", "Update the notes",
        "After each result or plan, the model rewrites its working notes ({{notes}}), which every attempt and "
        "plan sees. If that fails, the old notes stay.",
        [("what", "What happened", "upd", "in")],
        [("notes", "Notes", "save", "out")],
        [
            b("upd", "llm", "Rewrite the notes", 0, 0, {"prompt": NOTES, "temperature": 0.2, "max_tokens": 300},
              "Sees the notes so far and what just happened."),
            b("save", "store", "Save the notes", 300, 0, {"key": "notes", "max_chars": 2000},
              "Read anywhere as {{notes}}."),
            b("keep", "store", "Note the failure", 300, 200, {"key": "notes_error", "max_chars": 1000},
              "Keeps the old notes; just remembers what went wrong."),
        ],
        [l("upd", "out", "save", "in"), l("upd", "error", "keep", "in")],
    )


def a_attempt_fn():
    blocks = [
        b("entry", "forward", "Exchange", 0, 0),
        b("which", "read_exchange", "Which task", 0, -220, {"role": "user"}),
        b("work", "llm", "Try the task", 300, 0, {
            "system": A_ATTEMPT, "max_tokens": 800, "tools": TOOLS, "retries": 2, "network": True,
            "on_limit": "ask"},
            "Can use tools (all three switches must allow each one), answer, say TOO BIG, or ask a QUESTION."),
        b("check", "function", "Check the tool call", 300, 300, {"function": "check_call"},
          "Every call is checked before it runs."),
        b("run", "run_tool", "Run tool", 600, 300, {"max_result_chars": 4000},
          "Tool errors go back to the model, which can try again."),
        b("asks", "branch", "A question for you?", 600, 0, {"pattern": QUESTION}),
        b("ask", "ask_user", "Ask you", 900, -300, {
            "prompt": "_INPUT_", "timeout_minutes": 60, "default_answer": NO_ANSWER},
            "Waits up to an hour, then carries on without an answer."),
        b("no_answer", "combine", "No answer", 900, -150, {"inputs": 1, "format": NO_ANSWER}),
        b("keep", "remember", "Task exchange", 600, -450, {},
          "Re-sends the task's exchange each time a question or failure needs it."),
        b("q_ex", "augment", "Add the question", 1200, -450, {"role": "assistant", "format": "_INPUT_"}),
        b("q_ans", "augment", "Add your answer", 1200, -300, {"role": "user", "format": "_INPUT_"}),
        b("big_answer", "branch", "Too big? (answer)", 900, 0, {"pattern": TOO_BIG}),
        b("big_exchange", "branch", "Too big? (exchange)", 900, 200, {"pattern": TOO_BIG}),
        b("verify", "function", "Verify the result", 1200, 0, {"function": "verify"}),
        b("checked", "branch", "Passed the check?", 1200, 200, {"pattern": YES}),
        b("note", "augment", "Note the failed check", 1500, 450, {
            "role": "assistant", "format": "My attempt didn't pass this check: _INPUT_"}),
        b("add_result", "augment", "Add result", 1500, 150, {"role": "assistant", "format": "_INPUT_"}),
        b("message", "combine", "Result message", 1500, -150, {"inputs": 2, "format": "**✓ _INPUT1_**\n\n_INPUT2_"}),
        b("to_plan", "forward", "Needs a plan", 1800, 450),
        b("err_text", "combine", "Explain the failure", 900, 550, {
            "inputs": 1, "format": "I couldn't finish this task. (_INPUT1_)"},
            "If the model call fails, say so and move on."),
        b("err_add", "augment", "Add the failure", 1500, 650, {"role": "assistant", "format": "_INPUT_"}),
        b("done", "forward", "Exchange with result", 1800, 150),
    ]
    links = [
        l("entry", "out", "work", "in"), l("entry", "out", "which", "in"), l("entry", "out", "keep", "in"),
        l("work", "tool", "check", "exchange", "a tool call"),
        l("check", "approved", "run", "in"), l("check", "declined", "work", "in", "declined, with the reason"),
        l("run", "out", "work", "in", "results"),
        l("work", "out", "asks", "value"), l("work", "out", "asks", "test"),
        l("asks", "yes", "ask", "in", "QUESTION: …"), l("asks", "yes", "keep", "in"),
        l("ask", "error", "no_answer", "in1"), l("no_answer", "out", "q_ans", "in"),
        l("keep", "out", "q_ex", "exchange"), l("asks", "yes", "q_ex", "in"),
        l("q_ex", "out", "q_ans", "exchange"), l("ask", "out", "q_ans", "in"),
        l("q_ans", "out", "work", "in", "try again with your answer"),
        l("asks", "no", "big_answer", "value"), l("asks", "no", "big_answer", "test"),
        l("entry", "out", "big_exchange", "value"), l("asks", "no", "big_exchange", "test"),
        l("big_exchange", "yes", "to_plan", "in", "too big"),
        l("which", "last", "message", "in1"),
        l("which", "last", "verify", "request"), l("big_answer", "no", "verify", "answer", "a result"),
        l("big_exchange", "no", "checked", "value"), l("verify", "verdict", "checked", "test"),
        l("checked", "yes", "add_result", "exchange"), l("verify", "passed", "add_result", "in"),
        l("verify", "passed", "message", "in2"),
        l("checked", "no", "note", "exchange"), l("verify", "failed", "note", "in"),
        l("note", "out", "to_plan", "in", "failed its check"),
        l("work", "error", "err_text", "in1", "the call failed"), l("work", "error", "keep", "in"),
        l("err_text", "out", "message", "in2"), l("keep", "out", "err_add", "exchange"),
        l("err_text", "out", "err_add", "in"),
        l("add_result", "out", "done", "in"), l("err_add", "out", "done", "in"),
    ]
    return fn(
        "attempt", "Attempt the task",
        "The model tries the task, calling tools (each checked first) as often as it needs, or asks you a "
        "question. Its result is checked with Verify. If it says the task is too big, or the result fails its "
        "check, the exchange goes out of Breakdown instead. A failed model call becomes a short message.",
        [("exchange", "Exchange", "entry", "in")],
        [
            ("message", "Result message", "message", "out"),
            ("done", "Exchange with result", "done", "out"),
            ("breakdown", "Breakdown", "to_plan", "out"),
        ],
        blocks, links,
        [
            note("n1", 300, -600, "The work cycle: Try the task ⇄ Check the tool call ⇄ Run tool, until the model "
                                  "answers. A QUESTION: answer goes to you, then back to the model with your answer.",
                 420),
            note("n2", 900, 800, "Failures don't stop the run: a failed model call becomes “I couldn't finish this "
                                 "task”, and the work moves on.", 360),
        ],
    )


def a_plan_fn():
    f = plan_fn(True)
    f["blocks"] += [
        b("fail_text", "combine", "Explain the failure", 1040, 700, {
            "inputs": 1, "format": "(I couldn't break this task down: _INPUT1_)"},
            "If planning fails, say so; no steps are added."),
    ]
    f["links"] += [
        l("sub", "error", "fail_text", "in1", "planning failed"),
        l("again", "error", "fail_text", "in1", "planning failed"),
        l("fail_text", "out", "message", "in2"), l("fail_text", "out", "after", "in"),
    ]
    for blk in f["blocks"]:
        if blk["id"] == "sub":
            blk["comment"] = "Sees the running exchange, including the working notes."
        if blk["id"] == "again":
            blk["comment"] = "Only once, so a plan can't loop."
    return f


def assistant():
    blocks = [
        b("message", "input", "Your message", 0, 0),
        b("history", "history", "Recent conversation", 0, -250, {"max_messages": 10}),
        b("context", "combine", "Conversation and request", 300, -250, {
            "inputs": 2, "format": "Conversation so far:\n_INPUT1_\n\nThe user now says:\n_INPUT2_"}),
        b("clarify", "function", "Clarify the request", 600, 0, {"function": "clarify"},
          "Asks you one question if the request is unclear."),
        b("blank_notes", "fixed", "Empty notes", 0, 250, {"text": "(nothing yet)"}),
        b("start_notes", "store", "Start the notes", 300, 250, {"key": "notes", "max_chars": 2000}),
        b("goal_text", "combine", "Goal with context", 900, -250, {
            "inputs": 2, "format": "Conversation so far:\n_INPUT1_\n\nGoal: _INPUT2_"}),
        b("start", "exchange", "Running exchange", 1200, -250, {"system": A_SYSTEM, "roles": ["user"]},
          "The system message includes {{notes}}, read fresh at every model call."),
        b("queue", "forward", "Tasks to do", 900, 100),
        b("pair", "augment", "Next task", 1200, 0, {"role": "user", "format": "Task: _INPUT_"},
          "Waits for the running exchange and the next task, so tasks go strictly in order."),
        b("attempt", "function", "Attempt the task", 1500, 0, {"function": "attempt"}),
        b("show_result", "output", "Show result", 1800, -250),
        b("notes_result", "function", "Update the notes", 1800, -450, {"function": "update_notes"}),
        b("plan", "function", "Plan smaller steps", 1800, 200, {"function": "break_down"}),
        b("show_plan", "output", "Show plan", 2100, 200),
        b("notes_plan", "function", "Update the notes", 2100, 0, {"function": "update_notes"}),
        b("more", "for_each", "Up to 5 steps", 2100, 400, {"limit": 5},
          "New steps join the back of the queue: breadth-first."),
    ]
    links = [
        l("history", "out", "context", "in1"), l("message", "out", "context", "in2"),
        l("context", "out", "clarify", "context"), l("message", "out", "clarify", "request"),
        l("blank_notes", "out", "start_notes", "in"),
        l("history", "out", "goal_text", "in1"), l("clarify", "request", "goal_text", "in2"),
        l("clarify", "request", "queue", "in", "the first task: the request itself"),
        l("goal_text", "out", "start", "in"), l("start", "out", "pair", "exchange"),
        l("queue", "out", "pair", "in"), l("pair", "out", "attempt", "exchange"),
        l("attempt", "message", "show_result", "in"), l("attempt", "message", "notes_result", "what"),
        l("attempt", "done", "pair", "exchange", "the exchange, with the result"),
        l("attempt", "breakdown", "plan", "exchange", "too big"),
        l("plan", "message", "show_plan", "in"), l("plan", "message", "notes_plan", "what"),
        l("plan", "after", "pair", "exchange", "the exchange, with the plan"),
        l("plan", "steps", "more", "in"), l("more", "out", "queue", "in", "each step"),
    ]
    return {
        "format": "chummy.loop", "version": 2, "name": "Chummy assistant",
        "description": (
            "The default loop. It asks you a question first if your request is unclear, then tries it straight "
            "away, with tools. Anything too big is broken into 5 or fewer steps, tried the same way. Every tool "
            "call is checked before it runs, every result and plan is checked, and the model keeps working notes "
            "as it goes. Failures are caught and reported instead of stopping the run."
        ),
        "limits": {"max_steps": 2500, "max_llm_calls": 150, "max_tool_calls": 40},
        "blocks": blocks, "links": links,
        "functions": [a_verify_fn(), clarify_fn(), check_call_fn(), update_notes_fn(), a_attempt_fn(), a_plan_fn()],
        "notes": [
            note("n1", 0, 500,
                 "Chummy assistant\n\n1. Clarify: one question to you if the request is unclear.\n"
                 "2. Attempt: try each task now, with checked tool calls; ask you if stuck.\n"
                 "3. Plan: tasks that are too big become up to 5 steps, done breadth-first.\n"
                 "4. Notes: after every result and plan, the model updates its working notes ({{notes}}).\n\n"
                 "Double-click a function block to see inside.", 440, 230),
            note("n2", 1200, -450, "Each task pairs with the running exchange at Next task, so the model always "
                                   "sees the goal and everything done so far.", 380),
        ],
    }

for verify, file in [(False, "five-step-plan.chummy.json"), (True, "five-step-plan-verify.chummy.json")]:
    with open(os.path.join(HERE, file), "w", encoding="utf-8") as f:
        f.write(json.dumps(planner(verify), indent=2, ensure_ascii=False) + "\n")
with open(os.path.join(HERE, "assistant.chummy.json"), "w", encoding="utf-8") as f:
    f.write(json.dumps(assistant(), indent=2, ensure_ascii=False) + "\n")
print("wrote the planner templates and the Chummy assistant")
