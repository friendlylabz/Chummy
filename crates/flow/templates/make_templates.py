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

STEP = r"(?m)^\s*\d+[.)]\s*(.+?)\s*$"
TOO_BIG = r"(?i)^\W*too\s*big"
# A blank verdict (a thinking model that only thought, or ran out of room) passes:
# a check that says nothing never blocks the work.
YES = r"(?i)^\W*yes|^\s*$"

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
            "system": ATTEMPT, "retries": 2}),
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
        "format": "chummy.loop", "version": 3, "name": name, "description": description, "limits": limits,
        "blocks": blocks, "links": links, "functions": functions,
    }



# ---------------------------------------------------------------------------
# The Chummy assistant: the default loop, picked for you in onboarding.
#
#   Your message ─▶ Clarify ─▶ One go? ─yes─▶ Try ─▶ (check) ─▶ Reply
#                  (asks you     │             └ failed twice ─┐
#                   if unclear)  └─plan──────────────────────────┴▶ Make the full plan
#                                                                    │ (nothing is done yet)
#                                     Show the plan ◀─ Update notes ◀┘
#                                           └▶ Carry out the plan: each step, in order,
#                                              up to 5 checked tries; all fail ─▶ explain ─▶ Stop
#                                           └▶ Final summary (checked) ─▶ Reply
#
# - One go: the model first says whether it can do the whole goal in one go.
#   If so it tries (twice at most, each result checked); if that fails, it
#   plans instead.
# - Planning makes the whole plan before any work: at most 10 steps, checked
#   for being specific and concrete (re-planned once if not). Then, depth
#   first, the model says for each step whether it could do it in one go;
#   steps it can't are broken down the same way, at most two levels deep.
# - Carrying out: each step gets up to 5 tries with tools (every call checked
#   by the model first), and each result is checked. A step that fails all 5
#   gets an explanation from the model, and the run stops as failed.
# - Running context: {{goal}}, {{conversation}}, {{plan}} (the outline),
#   {{notes}} (rewritten by the model after one-go failures, the plan, and
#   each finished step) and {{results}} (the latest step results).

STEP_SYSTEM = (
    "You are a careful assistant doing one step of a larger piece of work for the user. Do exactly the step "
    "you're given, using tools when they help. Then give its result in a few sentences: what you did and what "
    "came out (names, values, files).\n\n"
    "The user's goal:\n{{goal}}\n\n"
    "Recent conversation:\n{{conversation}}\n\n"
    "The plan (✓ done, ○ to do, ▸ working, ▾ broken down):\n{{plan}}\n\n"
    "Your working notes:\n{{notes}}\n\n"
    "Results so far (latest last):\n{{results}}"
)
ONE_GO_Q = (
    "Recent conversation:\n_INPUT2_\n\nThe user's goal:\n_INPUT1_\n\n"
    "Could you do this whole goal in one go: one answer, using tools if needed, without first making a plan "
    "of several steps? Most everyday requests can be done in one go. "
    "Reply with only ONE GO or PLAN, then one short reason."
)
PLAN_RULES = (
    "Write a plan: the steps to do, in order. Rules:\n"
    "- Use as few steps as the work needs (often 2 to 5, never more than 10). Don't split anything that can "
    "be done in one go.\n"
    "- Make every step specific and concrete: say exactly what to do, with which files, names or values, and "
    "what it produces. No vague steps like \"research the topic\" or \"finalize\".\n"
    "- Each step must make sense on its own, without reading the others.\n"
    "Write only the steps, one per line, numbered like:\n1. First step\n2. Second step"
)
PLAN_CHECK = (
    "_INPUT1_\n\nThe plan:\n_INPUT2_\n\nCheck this plan. Would doing these steps in order get it done, is every "
    "step specific and concrete (it says exactly what to do and what it produces), and are there no needless "
    "tiny steps? Start with YES or NO, then give one short reason that says what to fix."
)
REPLAN = (
    "_INPUT1_\n\nA first plan was:\n_INPUT2_\n\nIts problem: _INPUT3_\n\n" + PLAN_RULES
)
STEP_JUDGE = (
    "The user's goal:\n{{goal}}\n\nThe whole plan:\n{{plan}}\n\nOne step of it: _INPUT1_\n\n"
    "Could you do this step in one go: one answer, using tools if needed? Most steps can. Only if it clearly "
    "needs several separate pieces of work, say BREAK DOWN. Reply with only ONE GO or BREAK DOWN, then one "
    "short reason."
)
SUB_BRIEF = (
    "The user's goal:\n{{goal}}\n\nThe whole plan so far:\n{{plan}}\n\n"
    "This step of the plan is too big for one go, so plan it in smaller steps: _INPUT1_"
)
EXPLAIN = (
    "The user's goal:\n{{goal}}\n\nThis step was tried 5 times and never passed its check:\n_INPUT1_\n\n"
    "In two or three sentences, tell the user why it couldn't be done and what would be needed to do it."
)
FINAL = (
    "The user's goal:\n{{goal}}\n\nThe plan:\n{{plan}}\n\nThe latest results:\n{{results}}\n\n"
    "Your working notes:\n{{notes}}\n\nAll the steps are done. Write the final reply to the user: the outcome "
    "and anything they need to know, in a few sentences. Don't repeat every step."
)
# Steps numbered 1 to 10 only: a plan has at most 10 steps.
STEP10 = r"(?m)^\s*(?:10|[1-9])[.)]\s*(.+?)\s*$"
ONE_GO = r"(?i)^\W*one\s*-?\s*go|^\s*$"
HAS_TEXT = r"\S"
LINES = r"[^\n]*\S[^\n]*"
TRIES = 5

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
# A blank reply counts as clear: an empty question is never put to you.
CLEAR = r"(?i)^\W*clear|^\s*$"
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
            b("clear", "branch", "Clear?", 300, 100, {"pattern": CLEAR}),
            b("unclear", "branch", "Question", 300, -100, {"pattern": CLEAR}),
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
        [("notes", "Notes", "save", "out"), ("after", "After", "after", "out")],
        [
            b("upd", "llm", "Rewrite the notes", 0, 0, {"prompt": NOTES, "temperature": 0.2, "max_tokens": 300},
              "Sees the notes so far and what just happened."),
            b("save", "store", "Save the notes", 300, 0, {"key": "notes", "max_chars": 2000},
              "Read anywhere as {{notes}}."),
            b("keep", "store", "Note the failure", 300, 200, {"key": "notes_error", "max_chars": 1000},
              "Keeps the old notes; just remembers what went wrong."),
            b("after", "forward", "After", 600, 100, {}, "Fires once either way, so the work can go on."),
        ],
        [l("upd", "out", "save", "in"), l("upd", "error", "keep", "in"),
         l("save", "out", "after", "in"), l("keep", "out", "after", "in")],
    )


def try_step_fn():
    """One try at a step: the model works on it with tools (every call checked first), then the result is
    verified. Passed sends the result; Failed sends what went wrong."""
    blocks = [
        b("brief", "forward", "What to do", 0, 0),
        b("step", "forward", "The step", 0, 400),
        b("start", "exchange", "Start the exchange", 260, 0, {"system": STEP_SYSTEM, "roles": ["user"]},
          "The system message holds the goal, the plan, the notes and the results so far, read fresh at every "
          "model call."),
        b("work", "llm", "Do the step", 520, 0, {"retries": 2, "on_limit": "ask"},
          "Uses the tools this agent allows."),
        b("check", "function", "Check the tool call", 520, 300, {"function": "check_call"},
          "Every call is checked before it runs."),
        b("run", "run_tool", "Run tool", 820, 300, {"max_result_chars": 4000},
          "Tool errors go back to the model, which can try again."),
        b("has_text", "branch", "Any answer?", 820, 0, {"pattern": HAS_TEXT},
          "An empty answer (a model that only thought) never passes."),
        b("empty", "combine", "Empty answer", 1080, 150, {"inputs": 1, "format": "The answer was empty."}),
        b("request", "combine", "What it should do", 820, 450, {
            "inputs": 1, "format": "The user's goal:\n{{goal}}\n\nThe step: _INPUT1_"}),
        b("verify", "function", "Verify the result", 1080, 0, {"function": "verify"},
          "The model writes a yes/no check question and answers it."),
        b("call_failed", "combine", "Model call failed", 820, -200, {
            "inputs": 1, "format": "The model call failed: _INPUT1_"}),
        b("tool_failed", "combine", "Tool failed", 1080, 450, {
            "inputs": 1, "format": "A tool couldn't run: _INPUT1_"}),
        b("passed", "forward", "Passed", 1340, 0),
        b("failed", "forward", "Failed", 1340, 250),
    ]
    links = [
        l("brief", "out", "start", "in"), l("start", "out", "work", "in"),
        l("work", "tool", "check", "exchange", "a tool call"),
        l("check", "approved", "run", "in"), l("check", "declined", "work", "in", "declined, with the reason"),
        l("run", "out", "work", "in", "results"),
        l("work", "out", "has_text", "value"), l("work", "out", "has_text", "test"),
        l("has_text", "no", "empty", "in1"), l("empty", "out", "failed", "in"),
        l("step", "out", "request", "in1"), l("request", "out", "verify", "request"),
        l("has_text", "yes", "verify", "answer", "the result"),
        l("verify", "passed", "passed", "in"), l("verify", "failed", "failed", "in", "didn't pass"),
        l("work", "error", "call_failed", "in1"), l("call_failed", "out", "failed", "in"),
        l("run", "error", "tool_failed", "in1"), l("tool_failed", "out", "failed", "in"),
    ]
    return fn(
        "try_step", "Try a step",
        "One try at a step: the model does it, calling tools (each checked first) as often as it needs, and the "
        "result is verified. Passed sends the result; Failed sends what went wrong (a failed check, an empty "
        "answer, or a failed model call or tool).",
        [("brief", "What to do", "brief", "in"), ("step", "The step", "step", "in")],
        [("passed", "Passed (result)", "passed", "out"), ("failed", "Failed (reason)", "failed", "out")],
        blocks, links,
        [note("n1", 0, -350, "The work cycle: Do the step ⇄ Check the tool call ⇄ Run tool, until the model "
                             "answers. Then Verify checks the result.", 420)],
    )


def do_step_fn():
    """Up to 5 tries at a step. Passed: show it, tick it, keep the result, update the notes. All failed: the
    model explains why, that's shown, and the run stops."""
    blocks = [
        b("step", "forward", "The step", 0, 0),
        b("working", "outline", "Mark as working", 0, -250, {"action": "mark", "status": "working"},
          "Shows ▸ in {{plan}} while it's being done."),
        b("brief1", "combine", "What to do", 260, 0, {"inputs": 1, "format": "Your step now: _INPUT1_"}),
    ]
    links = [l("step", "out", "working", "task"), l("step", "out", "brief1", "in1")]
    for i in range(1, TRIES + 1):
        x = 260 + i * 300
        blocks.append(b(f"try{i}", "function", f"Try {i} of {TRIES}", x, 0, {"function": "try_step"}))
        links += [l(f"brief{i}", "out", f"try{i}", "brief"), l("step", "out", f"try{i}", "step"),
                  l(f"try{i}", "passed", "result", "in", "passed")]
        blocks.append(b(f"brief{i + 1}", "combine", f"Add what went wrong ({i})", x + 150, 200, {
            "inputs": 2, "format": f"_INPUT1_\n\nTry {i} didn't pass: _INPUT2_" + (
                "\nFix that this time." if i < TRIES else "")}))
        links += [l(f"brief{i}", "out", f"brief{i + 1}", "in1"),
                  l(f"try{i}", "failed", f"brief{i + 1}", "in2", "failed")]
    last = f"brief{TRIES + 1}"
    blocks += [
        b("result", "forward", "Result", 2000, -300),
        b("message", "combine", "Result message", 2300, -450, {"inputs": 2, "format": "**✓ _INPUT1_**\n\n_INPUT2_"}),
        b("show", "output", "Show the result", 2600, -450),
        b("done", "outline", "Mark done", 2300, -650, {"action": "mark", "status": "done"}),
        b("add_result", "combine", "Add to the results", 2300, -300, {
            "inputs": 2, "format": "{{results}}\n\n✓ _INPUT1_\n_INPUT2_"}),
        b("results", "store", "Save the results", 2600, -300, {"key": "results", "max_chars": 4000},
          "Keeps the latest 4,000 characters. Read anywhere as {{results}}."),
        b("what", "combine", "What happened", 2300, -150, {
            "inputs": 2, "format": "Finished the step: _INPUT1_\nResult: _INPUT2_"}),
        b("notes", "function", "Update the notes", 2600, -150, {"function": "update_notes"}),
        b("ask_explain", "combine", "Ask why", 2000, 250, {"inputs": 1, "format": EXPLAIN}),
        b("explain", "llm", "Explain why", 2000, 400, {"temperature": 0.2},
          "The model tells you why the step couldn't be done."),
        b("no_explain", "combine", "If that fails too", 2000, 600, {
            "inputs": 2, "format": "I tried 5 times and it never passed its check.\n\n_INPUT1_"},
          "If the explanation fails, show what went wrong instead."),
        b("fail_message", "combine", "Failure message", 2300, 450, {
            "inputs": 2, "format": "**✗ I couldn't do this step: _INPUT1_**\n\n_INPUT2_\n\nI stopped here, so the "
                                   "rest of the plan wasn't done."}),
        b("show_fail", "output", "Show the failure", 2600, 450),
        b("failed", "outline", "Mark failed", 2300, 650, {"action": "mark", "status": "failed"}),
        b("reason", "combine", "The reason", 2600, 650, {
            "inputs": 2, "format": "Couldn't do the step “_INPUT1_”. _INPUT2_"}),
        b("stop", "stop", "Give up", 2900, 650, {},
          "Ends the run as failed, with the reason. Later steps don't run."),
    ]
    links += [
        l("step", "out", "message", "in1"), l("result", "out", "message", "in2"), l("message", "out", "show", "in"),
        l("step", "out", "add_result", "in1"), l("result", "out", "add_result", "in2"),
        l("add_result", "out", "results", "in"),
        l("step", "out", "what", "in1"), l("result", "out", "what", "in2"), l("what", "out", "notes", "what"),
        l(last, "out", "ask_explain", "in1", "all 5 failed"), l("ask_explain", "out", "explain", "in"),
        l(last, "out", "no_explain", "in1"), l("explain", "error", "no_explain", "in2"),
        l("step", "out", "fail_message", "in1"), l("explain", "out", "fail_message", "in2"),
        l("no_explain", "out", "fail_message", "in2"),
        l("fail_message", "out", "show_fail", "in"),
        l("step", "out", "reason", "in1"), l("explain", "out", "reason", "in2"),
        l("no_explain", "out", "reason", "in2"), l("reason", "out", "stop", "in"),
    ]
    # The outline is marked by the step's text, sent on only once there's a result (or a failure).
    blocks += [
        b("passed_task", "branch", "On success", 2000, -650, {"pattern": ANY}),
        b("failed_task", "branch", "On failure", 2000, 650, {"pattern": ANY}),
    ]
    links += [
        l("step", "out", "passed_task", "value"), l("result", "out", "passed_task", "test"),
        l("passed_task", "yes", "done", "task"),
        l("step", "out", "failed_task", "value"), l("fail_message", "out", "failed_task", "test"),
        l("failed_task", "yes", "failed", "task"),
    ]
    return fn(
        "do_step", "Do a step",
        f"Up to {TRIES} tries at a step, each with tools and a check. Each try sees what went wrong before. A "
        "step that passes is shown, ticked in the plan, added to {{results}}, and the notes are updated. A step "
        f"that fails all {TRIES} tries gets an explanation from the model, which is shown, and the run stops "
        "as failed.",
        [("step", "Step", "step", "in")],
        [("result", "Result", "result", "out")],
        blocks, links,
        [note("n1", 260, -500, f"Tries run left to right; each later try is told why the earlier ones didn't "
                               f"pass. The first passing try wins; the rest never run.", 420)],
    )


def make_plan_fn():
    """Writes a plan of up to 10 steps for a task, checks it, re-plans once if needed, and adds the steps to
    the outline under the task."""
    return fn(
        "make_plan", "Write a plan",
        "The model writes up to 10 specific steps, then checks the plan (does it get there, is every step "
        "concrete, no needless tiny steps). A plan that fails its check is written again once, with the problem. "
        "The steps go into the plan outline under the task. Failed sends the error if the model call fails.",
        [("brief", "What to plan", "brief", "in"), ("task", "Task in the outline", "task", "in")],
        [("steps", "Steps", "parse", "group1"), ("failed", "Failed", "failed", "out")],
        [
            b("brief", "forward", "What to plan", 0, 0),
            b("task", "forward", "Task", 0, 400),
            b("ask", "combine", "Ask for a plan", 260, 0, {"inputs": 1, "format": "_INPUT1_\n\n" + PLAN_RULES}),
            b("plan", "llm", "Write the plan", 520, 0, {"temperature": 0.3},
              "Only plans; nothing is done yet."),
            b("ask_check", "combine", "Ask about the plan", 780, -200, {"inputs": 2, "format": PLAN_CHECK}),
            b("check", "llm", "Is the plan good?", 1040, -200, {
                "system": "Answer YES or NO first, then give one short reason.", "temperature": 0.0},
              "Must start with YES or NO."),
            b("good", "branch", "Good?", 1300, 0, {"pattern": YES}),
            b("why", "branch", "Problem if not", 1300, -200, {"pattern": YES}),
            b("unchecked", "branch", "Check failed", 1300, 200, {"pattern": ANY},
              "If the check can't run, the plan is used as it is."),
            b("ask_again", "combine", "Ask again", 1560, -200, {"inputs": 3, "format": REPLAN}),
            b("again", "llm", "Write it again", 1820, -200, {"temperature": 0.3},
              "Only once, so planning can't loop."),
            b("final", "forward", "The plan", 2080, 0),
            b("parse", "capture", "Its steps (1–10)", 2340, 0, {"pattern": STEP10},
              "Only steps numbered 1 to 10 count."),
            b("outline", "outline", "Add to the outline", 2600, 0, {"action": "add_steps", "limit": 10},
              "Puts the steps under the task in {{plan}}."),
            b("failed", "forward", "Failed", 1040, 300),
        ],
        [
            l("brief", "out", "ask", "in1"), l("ask", "out", "plan", "in"),
            l("brief", "out", "ask_check", "in1"), l("plan", "out", "ask_check", "in2"),
            l("ask_check", "out", "check", "in"),
            l("plan", "out", "good", "value"), l("check", "out", "good", "test"),
            l("good", "yes", "final", "in", "good"),
            l("check", "out", "why", "value"), l("check", "out", "why", "test"),
            l("brief", "out", "ask_again", "in1"), l("plan", "out", "ask_again", "in2"),
            l("why", "no", "ask_again", "in3", "the problem"),
            l("ask_again", "out", "again", "in"), l("again", "out", "final", "in", "the new plan"),
            l("plan", "out", "unchecked", "value"), l("check", "error", "unchecked", "test"),
            l("unchecked", "yes", "final", "in"),
            l("again", "error", "unchecked", "test"),
            l("final", "out", "parse", "in"),
            l("task", "out", "outline", "task"), l("parse", "group1", "outline", "steps"),
            l("plan", "error", "failed", "in", "couldn't plan"),
        ],
    )


def plan_step_fn():
    """Depth first: the model says whether a step can be done in one go. If so, it becomes a step to do;
    if not, it gets its own plan (one level down), whose steps become the steps to do."""
    return fn(
        "plan_step", "Plan a step",
        "The model says whether this step can be done in one go. If it can, the step is added to the steps to "
        "do ({{steps}}). If not, it gets its own plan of up to 10 steps, and those are added instead. A failed "
        "judgement or plan keeps the step as it is.",
        [("step", "Step", "step", "in")],
        [],
        [
            b("step", "forward", "Step", 0, 0),
            b("ask", "combine", "Ask about the step", 260, -200, {"inputs": 1, "format": STEP_JUDGE}),
            b("judge", "llm", "One go?", 520, -200, {
                "system": "Reply ONE GO or BREAK DOWN, then one short reason.", "temperature": 0.0},
              "A blank answer counts as one go."),
            b("one_go", "branch", "One go", 780, 0, {"pattern": ONE_GO}),
            b("judge_failed", "branch", "Judgement failed", 780, 200, {"pattern": ANY},
              "If the model call fails, keep the step as it is."),
            b("sub_brief", "combine", "Plan this step", 1040, -200, {"inputs": 1, "format": SUB_BRIEF}),
            b("sub", "function", "Write a plan", 1300, -200, {"function": "make_plan"},
              "One level down: these steps aren't broken down again."),
            b("joined", "join", "One per line", 1560, -200, {"separator": "\\n"}),
            b("any", "branch", "Any steps?", 1820, -200, {"pattern": HAS_TEXT}),
            b("none", "branch", "No steps", 1820, 0, {"pattern": HAS_TEXT},
              "A plan with no numbered steps keeps the step as it is."),
            b("plan_failed", "branch", "Plan failed", 1560, 100, {"pattern": ANY}),
            b("add", "combine", "Add to the steps", 2080, 0, {"inputs": 1, "format": "{{steps}}\n_INPUT1_"}),
            b("save", "store", "Save the steps", 2340, 0, {"key": "steps", "max_chars": 50000},
              "The steps to do, one per line, in order."),
        ],
        [
            l("step", "out", "ask", "in1"), l("ask", "out", "judge", "in"),
            l("step", "out", "one_go", "value"), l("judge", "out", "one_go", "test"),
            l("one_go", "yes", "add", "in1", "one go: do it as it is"),
            l("step", "out", "judge_failed", "value"), l("judge", "error", "judge_failed", "test"),
            l("judge_failed", "yes", "add", "in1"),
            l("one_go", "no", "sub_brief", "in1", "break down"),
            l("sub_brief", "out", "sub", "brief"), l("one_go", "no", "sub", "task"),
            l("sub", "steps", "joined", "in"),
            l("joined", "out", "any", "value"), l("joined", "out", "any", "test"),
            l("any", "yes", "add", "in1", "its steps"),
            l("one_go", "no", "none", "value"), l("joined", "out", "none", "test"),
            l("none", "no", "add", "in1"),
            l("one_go", "no", "plan_failed", "value"), l("sub", "failed", "plan_failed", "test"),
            l("plan_failed", "yes", "add", "in1"),
            l("add", "out", "save", "in"),
        ],
    )


def full_plan_fn():
    """The whole planning phase: a plan for the goal, then each of its steps planned in turn, depth first.
    Nothing is done yet. Planned fires once, after all of it."""
    return fn(
        "full_plan", "Make the full plan",
        "Plans the goal in up to 10 steps, then goes through them in order: each step the model can't do in one "
        "go gets its own plan right away (depth first, at most two levels). Nothing is done yet. The steps to do "
        "end up in {{steps}}, one per line, and the outline in {{plan}}. Planned fires once, when all of it is "
        "finished.",
        [("goal", "Goal", "goal", "in")],
        [("planned", "Planned", "goal", "out")],
        [
            b("goal", "forward", "Goal", 0, 0),
            b("brief", "combine", "What to plan", 260, 0, {
                "inputs": 1, "format": "Recent conversation:\n{{conversation}}\n\nThe user's goal:\n_INPUT1_\n\n"
                                       "Your working notes:\n{{notes}}"}),
            b("plan", "function", "Write a plan", 520, 0, {"function": "make_plan"}),
            b("each", "for_each", "Each step, in order", 780, 0, {"limit": 10}),
            b("step", "function", "Plan a step", 1040, 0, {"function": "plan_step"},
              "Runs once per step, finishing each (and its own plan) before the next: depth first."),
            b("failed", "store", "Note the failure", 780, 250, {"key": "plan_error", "max_chars": 1000},
              "If planning fails, there are no steps and the goal is done as one step."),
        ],
        [
            l("goal", "out", "brief", "in1"), l("brief", "out", "plan", "brief"), l("goal", "out", "plan", "task"),
            l("plan", "steps", "each", "in"), l("each", "out", "step", "step", "each step"),
            l("plan", "failed", "failed", "in"),
        ],
    )


def carry_out_fn():
    return fn(
        "carry_out", "Carry out the plan",
        "Does the steps to do one at a time, in order, each with Do a step. Finished fires once, after the last "
        "step. If a step fails all its tries, the run stops there.",
        [("steps", "Steps (one per line)", "steps", "in")],
        [("finished", "Finished", "steps", "out")],
        [
            b("steps", "forward", "Steps", 0, 0),
            b("lines", "regex", "One per line", 260, 0, {"pattern": LINES}),
            b("each", "for_each", "Each step, in order", 520, 0, {"limit": 0}),
            b("do", "function", "Do a step", 780, 0, {"function": "do_step"},
              "Each step finishes (result, notes) before the next starts."),
        ],
        [
            l("steps", "out", "lines", "in"), l("lines", "out", "each", "in"), l("each", "out", "do", "step"),
        ],
    )


def assistant():
    blocks = [
        b("message", "input", "Your message", 0, 0),
        b("history", "history", "Recent conversation", 0, -250, {"max_messages": 10}),
        b("context", "combine", "Conversation and request", 300, -250, {
            "inputs": 2, "format": "Conversation so far:\n_INPUT1_\n\nThe user now says:\n_INPUT2_"}),
        b("clarify", "function", "Clarify the request", 300, 0, {"function": "clarify"},
          "Asks you one question if the request is unclear."),
        b("goal", "store", "Remember the goal", 600, -250, {"key": "goal", "max_chars": 8000}),
        b("conversation", "store", "Remember the conversation", 300, -450, {"key": "conversation", "max_chars": 6000}),
        b("blank_notes", "fixed", "Empty notes", 0, 250, {"text": "(nothing yet)"}),
        b("start_notes", "store", "Start the notes", 300, 250, {"key": "notes", "max_chars": 2000}),
        b("start_plan", "outline", "Start the plan outline", 600, 250, {"action": "start"},
          "The goal is the root of {{plan}}."),
        # One go?
        b("ask_one_go", "combine", "Ask: one go?", 600, 0, {"inputs": 2, "format": ONE_GO_Q}),
        b("one_go_judge", "llm", "One go?", 900, 0, {
            "system": "Reply ONE GO or PLAN, then one short reason.", "temperature": 0.0},
          "A blank answer counts as one go."),
        b("one_go", "branch", "One go", 1200, 0, {"pattern": ONE_GO}),
        b("judge_failed", "branch", "Judgement failed", 1200, 200, {"pattern": ANY},
          "If the model call fails, try it in one go."),
        b("go", "forward", "Do it in one go", 1500, 0),
        b("brief1", "combine", "What to do", 1800, 0, {"inputs": 1, "format": "Your task now: _INPUT1_"}),
        b("try1", "function", "Try 1 of 2", 2100, 0, {"function": "try_step"}),
        b("brief2", "combine", "Add what went wrong", 2400, 200, {
            "inputs": 2, "format": "_INPUT1_\n\nTry 1 didn't pass: _INPUT2_\nFix that this time."}),
        b("try2", "function", "Try 2 of 2", 2700, 200, {"function": "try_step"}),
        b("answer", "forward", "Answer", 3000, -100),
        b("show_answer", "output", "Show the answer", 3300, -100),
        b("answer_done", "branch", "Only on success", 3000, -300, {"pattern": ANY}),
        b("mark_done", "outline", "Mark done", 3300, -300, {"action": "mark", "status": "done"}),
        b("why_plan", "combine", "Why plan instead", 3000, 400, {
            "inputs": 1, "format": "I tried to do it in one go, but it didn't pass its check: _INPUT1_"}),
        b("say_plan", "combine", "Tell you", 3300, 400, {
            "inputs": 1, "format": "↻ That didn't work in one go, so I'm making a plan first."}),
        b("show_replan", "output", "Show that", 3600, 400),
        b("notes_one_go", "function", "Update the notes", 3300, 600, {"function": "update_notes"}),
        b("goal_again", "branch", "Then plan", 3600, 600, {"pattern": ANY}),
        # Planning
        b("to_plan", "forward", "Plan", 1500, 900),
        b("full_plan", "function", "Make the full plan", 1800, 900, {"function": "full_plan"},
          "Nothing is done while planning."),
        b("planned_steps", "recall", "Any steps?", 2100, 900, {"key": "steps"}),
        b("got_steps", "branch", "Got steps", 2400, 900, {"pattern": HAS_TEXT}),
        b("plan_text", "combine", "The plan", 2100, 750, {
            "inputs": 1, "format": "**▾ Here's the plan:**\n{{plan}}"}),
        b("show_plan", "output", "Show the plan", 2400, 750),
        b("plan_what", "combine", "What happened", 2100, 1050, {
            "inputs": 1, "format": "Made this plan (nothing done yet):\n{{plan}}"}),
        b("notes_plan", "function", "Update the notes", 2400, 1050, {"function": "update_notes"},
          "The notes are updated before any step starts."),
        b("steps", "recall", "The steps to do", 2700, 1050, {"key": "steps"}),
        b("no_steps", "branch", "No steps", 2400, 1250, {"pattern": r"^\s*$"},
          "If planning produced nothing, the goal is done as one step."),
        b("say_one", "combine", "Tell you", 2700, 1250, {
            "inputs": 1, "format": "▾ I couldn't break this into steps, so I'll do it as one."}),
        b("show_one", "output", "Show that", 3000, 1350),
        b("then_one", "branch", "Then do it", 3000, 1200, {"pattern": ANY}),
        # Carrying out
        b("carry_out", "function", "Carry out the plan", 3300, 900, {"function": "carry_out"}),
        b("as_one", "function", "Do it as one step", 3300, 1200, {"function": "do_step"}),
        b("ask_final", "combine", "Ask for the final reply", 3600, 750, {"inputs": 1, "format": FINAL}),
        b("final", "llm", "Write the final reply", 3600, 900, {"temperature": 0.3}),
        b("final_request", "combine", "The goal", 3600, 1100, {"inputs": 1, "format": "{{goal}}"}),
        b("final_check", "function", "Check the reply", 3900, 1000, {"function": "verify"}),
        b("final_ok", "combine", "Final message", 4200, 900, {"inputs": 1, "format": "**Done.** _INPUT1_"}),
        b("final_doubt", "combine", "With the doubt", 4200, 1100, {
            "inputs": 2, "format": "**Done.** _INPUT1_\n\n(My own check of this isn't sure: _INPUT2_)"}),
        b("final_failed", "combine", "If that fails", 3900, 1300, {
            "inputs": 1, "format": "**Done.** All the steps above are finished."}),
        b("show_final", "output", "Show the final reply", 4500, 1000),
    ]
    links = [
        l("history", "out", "context", "in1"), l("message", "out", "context", "in2"),
        l("context", "out", "clarify", "context"), l("message", "out", "clarify", "request"),
        l("history", "out", "conversation", "in"),
        l("clarify", "request", "goal", "in"),
        l("blank_notes", "out", "start_notes", "in"),
        l("clarify", "request", "start_plan", "in"),
        l("clarify", "request", "ask_one_go", "in1"), l("history", "out", "ask_one_go", "in2"),
        l("ask_one_go", "out", "one_go_judge", "in"),
        l("clarify", "request", "one_go", "value"), l("one_go_judge", "out", "one_go", "test"),
        l("one_go", "yes", "go", "in", "one go"), l("one_go", "no", "to_plan", "in", "plan"),
        l("clarify", "request", "judge_failed", "value"), l("one_go_judge", "error", "judge_failed", "test"),
        l("judge_failed", "yes", "go", "in"),
        l("go", "out", "brief1", "in1"), l("brief1", "out", "try1", "brief"), l("go", "out", "try1", "step"),
        l("brief1", "out", "brief2", "in1"), l("try1", "failed", "brief2", "in2", "failed"),
        l("brief2", "out", "try2", "brief"), l("go", "out", "try2", "step"),
        l("try1", "passed", "answer", "in", "passed"), l("try2", "passed", "answer", "in", "passed"),
        l("answer", "out", "show_answer", "in"),
        l("go", "out", "answer_done", "value"), l("answer", "out", "answer_done", "test"),
        l("answer_done", "yes", "mark_done", "task"),
        l("try2", "failed", "why_plan", "in1", "failed twice"),
        l("why_plan", "out", "say_plan", "in1"), l("say_plan", "out", "show_replan", "in"),
        l("why_plan", "out", "notes_one_go", "what"),
        l("go", "out", "goal_again", "value"), l("notes_one_go", "after", "goal_again", "test"),
        l("goal_again", "yes", "to_plan", "in", "plan instead"),
        l("to_plan", "out", "full_plan", "goal"),
        l("full_plan", "planned", "planned_steps", "in"),
        l("planned_steps", "out", "got_steps", "value"), l("planned_steps", "out", "got_steps", "test"),
        l("got_steps", "yes", "plan_text", "in1", "a plan"), l("plan_text", "out", "show_plan", "in"),
        l("got_steps", "yes", "plan_what", "in1"), l("plan_what", "out", "notes_plan", "what"),
        l("notes_plan", "after", "steps", "in", "then start"),
        l("steps", "out", "carry_out", "steps"),
        l("to_plan", "out", "no_steps", "value"), l("planned_steps", "out", "no_steps", "test"),
        l("no_steps", "yes", "say_one", "in1", "no plan came out"), l("say_one", "out", "show_one", "in"),
        l("to_plan", "out", "then_one", "value"), l("say_one", "out", "then_one", "test"),
        l("then_one", "yes", "as_one", "step"),
        l("carry_out", "finished", "ask_final", "in1", "all steps done"), l("ask_final", "out", "final", "in"),
        l("carry_out", "finished", "final_request", "in1"),
        l("final_request", "out", "final_check", "request"), l("final", "out", "final_check", "answer"),
        l("final_check", "passed", "final_ok", "in1"), l("final_ok", "out", "show_final", "in"),
        l("final", "out", "final_doubt", "in1"), l("final_check", "failed", "final_doubt", "in2"),
        l("final_doubt", "out", "show_final", "in"),
        l("final", "error", "final_failed", "in1"), l("final_failed", "out", "show_final", "in"),
    ]
    return {
        "format": "chummy.loop", "version": 3, "name": "Chummy assistant",
        "description": (
            "The default loop. It asks you a question first if your request is unclear. If the model can do it "
            "in one go, it does, with its result checked. Otherwise it makes the whole plan first (up to 10 "
            "specific steps, big steps planned again, two levels at most), then does the steps in order, each "
            "with up to 5 checked tries. Every tool call is checked before it runs, and the model keeps working "
            "notes. If a step fails all its tries, it says why and stops."
        ),
        "limits": {"max_steps": 10000, "max_llm_calls": 400, "max_tool_calls": 120},
        "blocks": blocks, "links": links,
        "functions": [a_verify_fn(), clarify_fn(), check_call_fn(), update_notes_fn(), try_step_fn(),
                      do_step_fn(), make_plan_fn(), plan_step_fn(), full_plan_fn(), carry_out_fn()],
        "notes": [
            note("n1", 0, 500,
                 "Chummy assistant\n\n1. Clarify: one question to you if the request is unclear.\n"
                 "2. One go? If the model can do it all at once, it tries (twice at most, each result "
                 "checked). If that fails, it plans.\n"
                 "3. Plan: the whole plan first, nothing done yet. Up to 10 specific steps, checked; each step "
                 "too big for one go gets its own plan, depth first, two levels at most.\n"
                 "4. Carry out: each step in order, up to 5 tries, every tool call and result checked. A step "
                 "that fails all 5 is explained, and the run stops.\n"
                 "5. Context: {{goal}}, {{plan}}, {{notes}} (rewritten by the model after the plan and each "
                 "step) and {{results}}.\n\n"
                 "Double-click a function block to see inside.", 460, 300),
        ],
    }

def tidy(loop):
    """Model blocks that do the work inherit the agent's tools and every model setting;
    the others (judges, checks, plans, notes) never get tools."""
    def walk(blocks):
        for blk in blocks:
            if blk["type"] != "llm":
                continue
            p = blk.setdefault("params", {})
            p.pop("max_tokens", None)
            if blk["id"] != "work":
                p["tools"] = "off"
    walk(loop["blocks"])
    for f in loop.get("functions", []):
        walk(f["blocks"])
    return loop


for verify, file in [(False, "five-step-plan.chummy.json"), (True, "five-step-plan-verify.chummy.json")]:
    with open(os.path.join(HERE, file), "w", encoding="utf-8") as f:
        f.write(json.dumps(tidy(planner(verify)), indent=2, ensure_ascii=False) + "\n")
with open(os.path.join(HERE, "assistant.chummy.json"), "w", encoding="utf-8") as f:
    f.write(json.dumps(tidy(assistant()), indent=2, ensure_ascii=False) + "\n")
print("wrote the planner templates and the Chummy assistant")
