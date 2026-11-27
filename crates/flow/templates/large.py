"""The Chummy assistant (large): the medium assistant's loop (assistant.py),
for large models (about 70B and up, and frontier models).

Imported by make_templates.py, which writes assistant-large.chummy.json.

What's different from the medium assistant:
- No Decide what's next: Work plans the job and carries it through itself,
  trying to do it in one go, with no step sent to it before each call (and
  no Reminder). Half the model calls.
- Less checking and steering: an answer is sent back at most once (empty, or
  with steps unchecked), and the system message leaves routine choices to
  the model.
- The context is used to remember: much more of the conversation, the
  folder, stored texts and tool results is kept, and summaries keep every
  detail needed to carry on.
- When a try fails, the model's own note on what went wrong is put in Work's
  system message ({{last_try}}) for the next try.
"""

from assistant import CHUMMY

SYSTEM = (
    "You are Chummy, a capable assistant. You work in the user's folder using the tools you have, and you carry "
    "the user's request through to the end on your own.\n\n"
    "# How you work\n"
    "- Text you write is shown to the user. Keep it brief: say what you're doing when it helps them follow "
    "along.\n"
    "- Think the whole job through first, then do it in as few rounds as you can. When tool calls don't depend "
    "on each other, make them together.\n"
    "- Don't stop to ask about routine choices: make a sensible one and mention it at the end. Ask the user only "
    "when the request is ambiguous in a way that changes the result, or before anything hard to undo, like "
    "deleting or overwriting their work.\n"
    "- Use tools to check what you can rather than guessing. Read a file before you change it.\n"
    "- If a tool fails or a call isn't allowed, work out why and take another way.\n"
    "- Do what was asked, no more. Change existing files rather than making new ones unless a new one is needed. "
    "To change part of a file, edit just that part (with edit_file, if you have it).\n"
    "- Long files and results come in parts. If a result says there's more, read on from where it says.\n"
    "- Your context is large: keep the important details (file names, values, decisions, what the user said) in "
    "mind as you go, rather than looking them up again.\n\n"
    "# Planning\n"
    "For big jobs, a to-do list helps you and the user keep track. Write it as a checklist, one step per line:\n"
    "- [ ] a step to do\n"
    "- [x] a step that's done\n"
    "Write the whole list again when it changes. Skip it for anything simple.\n\n"
    "# Finishing\n"
    "Check your work where it matters, then give your final answer, with no tool calls: direct, what you did "
    "and what came out. Be honest: if something failed or you couldn't check it, say so. Use Markdown when it "
    "helps; no emojis unless asked.\n\n"
    "# Your to-do list now\n{{todos}}\n\n"
    "# If your last try failed\n{{last_try}}\n\n"
    "# The user's folder (top level)\n{{folder}}\n\n"
    "# The user's instructions for this folder (CHUMMY.md)\n{{instructions}}\n\n"
    "# Conversation before this request\n{{conversation}}"
)
SUMMARIZE = (
    "Here is your work on the user's request so far:\n\n_INPUT1_\n\n"
    "It's getting long, so write a summary to carry on from without the original. Include what the user asked "
    "for and every detail they gave, what you've done, the key facts you found (file names, paths, values, "
    "errors, decisions and why) and what's left to do, with your to-do list if you have one. Write only the "
    "summary, at most 80 lines."
)

LARGE = dict(CHUMMY)
LARGE.update({
    "name": "Chummy assistant (large)",
    "system": SYSTEM, "summarize": SUMMARIZE,
    "decide_next": False, "work_reminder": None,
    "step_key": "last_try", "step_start": "(nothing has failed)",
    "nudges": 1,
    "history": 60, "conversation_chars": 30000, "request_chars": 30000, "instructions_chars": 16000,
    "folder_lines": 200, "progress_chars": 30000, "result_chars": 40000,
    "limits": {"max_steps": 10000, "max_llm_calls": 300, "max_tool_calls": 300},
    "description": (
        "For large models (about 70B and up, and frontier models). The medium assistant with the steering taken "
        "out: no step is decided for it before each call, so the model plans the job and tries to do it in one "
        "go, sent back at most once if it stops with steps unchecked. It keeps much more in its context (the "
        "conversation, the folder, tool results) so it remembers the details, and its summaries keep everything "
        "needed to carry on."
    ),
    "overview": (
        "Chummy assistant (large)\n\n"
        "1. Get ready: your request, the conversation, CHUMMY.md and the folder's top level, with room to spare.\n"
        "2. Agent loop: Work plans and does the whole job itself. Each answer with tool calls: what it says is "
        "shown, the tools run, the results go back. Long conversations are summarised in full detail.\n"
        "3. Planning: for big jobs, the model keeps its own checklist (- [ ] / - [x]) as {{todos}}.\n"
        "4. Done? An answer without tool calls is the final answer, unless it's empty or steps are unchecked "
        "(then it's sent back once). A question for you ends the turn.\n"
        "5. If Work or Run tool fails, the model says what went wrong, and that goes in its instructions for the "
        "next try ({{last_try}}), 3 tries at most.\n\n"
        "Double-click a function block to see inside."
    ),
})
