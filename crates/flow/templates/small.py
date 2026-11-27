"""The Chummy assistant (small): the medium assistant's loop (assistant.py),
for small models (about 1B to 14B).

Imported by make_templates.py, which writes assistant-small.chummy.json.

What's different from the medium assistant:
- More steering: Decide what's next also says when to check the last change
  and when to ask the user, and the Reminder repeats that.
- Self-checks: before an answer is final, a small tool-less call checks it
  against the request and the work. A problem sends the model back to fix it
  (within the usual two times), and the check is shown in the run's pane.
- Asking: when the request is unclear or the work goes in circles, the model
  is told to ask you one short question rather than guess.
- A light context: fewer earlier messages, shorter stored texts and tool
  results, a shorter folder listing and summaries, and room is made at 60% of
  the context instead of the model's own point.
- A shorter, plainer system message.
"""

from assistant import CHUMMY

SYSTEM = (
    "You are Chummy, a helpful assistant. You work in the user's folder with the tools you have, one step at a "
    "time, until the user's request is done.\n\n"
    "# Rules\n"
    "- Do the step below, and only that step.\n"
    "- Before you call a tool, say in one short sentence what you're about to do.\n"
    "- Read a file before you change it. After every change, check it: read the file back, or run it.\n"
    "- If a tool fails, read the error and try another way. Never repeat the same call.\n"
    "- If you're not sure what the user wants, or you're stuck, ask the user one short question and stop. "
    "Don't guess.\n"
    "- Before deleting or overwriting the user's work, ask the user first.\n"
    "- Do what was asked, no more.\n\n"
    "# Planning\n"
    "For work with 3 or more steps, keep a to-do list. Write it as a checklist, one step per line:\n"
    "- [ ] a step to do\n"
    "- [x] a step that's done\n"
    "Write the whole list again each time you finish a step. For simple requests, skip the list.\n\n"
    "# Finishing\n"
    "When everything is done and checked, give your final answer, with no tool calls: short, what you did and "
    "what came out. If something failed or you couldn't check it, say so.\n\n"
    "# Your to-do list now\n{{todos}}\n\n"
    "# Your current step\n{{next}}\n\n"
    "# The user's folder (top level)\n{{folder}}\n\n"
    "# The user's instructions for this folder (CHUMMY.md)\n{{instructions}}\n\n"
    "# Conversation before this request\n{{conversation}}"
)
SUMMARIZE = (
    "Here is your work on the user's request so far:\n\n_INPUT1_\n\n"
    "It's getting long, so write a short summary to carry on from: what the user asked for, what's done, the "
    "key facts (file names, values, errors) and what's left, with your to-do list if you have one. Write only "
    "the summary, at most 15 short lines."
)
NEXT = (
    "You are directing the work on the user's request:\n{{request}}\n\n"
    "The to-do list:\n{{todos}}\n\n"
    "The step being worked on until now:\n{{next}}\n\n"
    "The latest part of the work (the newest is at the end):\n_INPUT1_\n\n"
    "Decide the ONE next step to take now to finish the user's request. Choose the first that fits:\n"
    "1. If the last step changed a file and that wasn't checked yet, the step is to check it.\n"
    "2. If the request is unclear, or the same thing has failed twice, the step is to ask the user: write "
    "\"Ask the user: \" and one short question.\n"
    "3. If the to-do list has an unchecked item, the step is the first one.\n"
    "4. If the request is fully done and checked, write: \"Give the user the final answer now.\"\n"
    "Otherwise, the step that gets the request done. Stay on the request: no side tasks. Write the step as a "
    "direct instruction in one sentence of at most 25 words, starting with a verb and naming the file, tool or "
    "fact it's about, for example: \"Read notes.txt and find this month's total.\" Write only that sentence."
)
WORK_REMINDER = (
    "(A note from your own planning, not from the user.) Your step now: {{next}}\n"
    "Do just this step: call a tool if it needs one. If the step is to ask the user, ask your one question and "
    "stop. If the whole request is done and checked, give the final answer."
)
SELF_CHECK = (
    "The user's request:\n{{request}}\n\n"
    "The latest work on it:\n{{progress}}\n\n"
    "The answer about to be given:\n_INPUT1_\n\n"
    "Check the answer. Does it do what the user asked? Does the work back up what it says? Reply OK if it's "
    "right. If not, reply with one sentence that starts with \"Problem:\" and says what's wrong or missing."
)
CHECK_NUDGE = (
    "The user's request:\n{{request}}\n\n"
    "A check of your answer found: {{check}}\n\n"
    "Fix that now (use tools if you need to), then give your final answer again. If the check is wrong, give "
    "the final answer and say why."
)

SMALL = dict(CHUMMY)
SMALL.update({
    "name": "Chummy assistant (small)",
    "system": SYSTEM, "summarize": SUMMARIZE, "next": NEXT, "work_reminder": WORK_REMINDER,
    "self_check": SELF_CHECK, "check_nudge": CHECK_NUDGE,
    "history": 6, "conversation_chars": 2000, "request_chars": 4000, "instructions_chars": 2000,
    "folder_lines": 30, "progress_chars": 3000, "result_chars": 4000, "recent_chars": 1500,
    "compact_percent": 60,
    # Decide what's next before every Work call, and a check before each final answer.
    "limits": {"max_steps": 10000, "max_llm_calls": 400, "max_tool_calls": 100},
    "description": (
        "For small models (about 1B to 14B). The medium assistant with more help: before each call it decides "
        "the one next step, including checking the last change or asking you when the request is unclear or "
        "it's going in circles, and a small call checks the answer before it's final and sends the model back "
        "to fix what's wrong. It keeps its context light: fewer earlier messages, shorter tool results and "
        "summaries, and it makes room early (at 60% of the context)."
    ),
    "overview": (
        "Chummy assistant (small)\n\n"
        "1. Get ready: your request, a little of the conversation, CHUMMY.md and the folder's top level.\n"
        "2. Agent loop: one step at a time. Before each call, Decide what's next picks the step (check the last "
        "change, ask you, or the next to-do) and Work is told to do just that.\n"
        "3. Planning: the model keeps its own checklist (- [ ] / - [x]) as {{todos}}.\n"
        "4. Done? Empty answers and unchecked steps are sent back; then a check of the answer sends it back if "
        "something's wrong (twice at most). A question for you ends the turn.\n"
        "5. Light context: short results and summaries, and room is made at 60% of the context.\n\n"
        "Double-click a function block to see inside."
    ),
})
