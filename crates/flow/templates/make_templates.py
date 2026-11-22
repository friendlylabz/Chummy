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


def b(id, type, label, x, y, params=None):
    return {"id": id, "type": type, "label": label, "position": {"x": x, "y": y}, "params": params or {}}


def l(a, ap, c, cp):
    return {"from": {"block": a, "port": ap}, "to": {"block": c, "port": cp}}


def fn(id, name, description, inputs, outputs, blocks, links):
    return {
        "id": id,
        "name": name,
        "description": description,
        "inputs": [{"name": n, "label": lab, "to": {"block": blk, "port": p}} for n, lab, blk, p in inputs],
        "outputs": [{"name": n, "label": lab, "from": {"block": blk, "port": p}} for n, lab, blk, p in outputs],
        "blocks": blocks,
        "links": links,
    }


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
        "format": "chummy.loop", "version": 1, "name": name, "description": description, "limits": limits,
        "blocks": blocks, "links": links, "functions": functions,
    }


for verify, file in [(False, "five-step-plan.chummy.json"), (True, "five-step-plan-verify.chummy.json")]:
    with open(os.path.join(HERE, file), "w", encoding="utf-8") as f:
        f.write(json.dumps(planner(verify), indent=2, ensure_ascii=False) + "\n")
print("wrote the planner templates")
