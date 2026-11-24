# Loops and the loop editor

## What a loop is

A **loop** is how an agent thinks: a graph of small **blocks** joined by
**links**. Your message enters at a *Message* block, text flows along the
links from block to block, and whatever reaches a *Reply* block is the
agent's answer.

- A block runs when **every input has a value**, then sends its result to
  everything linked to its outputs.
- Links can go backwards, so a loop can **cycle**: draft, check, try again.
  A *Branch* leaves the cycle, and the loop's limits (steps, model calls,
  tool calls) stop it if it never does.
- Blocks that can fail (model calls, tools, files, Ask me) have a
  **Failed** output. Link it and the loop handles the failure; leave it
  unlinked and the failure stops the run.

The blocks you'll use most:

| Block | What it does |
|---|---|
| **Ask the model** | Sends a prompt (with `_INPUT_` where the input goes) and outputs the Answer, a Tool call, the Reasoning, or Failed. With an exchange, its **Reminder** is sent last for that call only: a good place for "do this step now". |
| **Run tool** | Runs the tool calls the model asked for and adds the results to the conversation. |
| **Start / Add to / Read exchange** | Build and read a conversation with the model that flows between blocks. |
| **Combine**, **Split**, **Join**, **Find matches** | Put text together, cut it up, or pick parts out. |
| **Branch** | Sends a value one way or the other, depending on whether a test text matches a pattern (e.g. starts with "yes"). |
| **For each** | Sends each item of a list on its own. |
| **Save to / Read memory** | Keep text under a name for the rest of the run; any prompt can include it as `{{name}}`. |
| **Ask me** | Asks you a question and waits for your answer. |
| **Read / Write / List files** | Work with files in the agent's folder. |
| **Add to chat output** | Adds its input to the run's response in the chat; the run carries on. |
| **Final chat output** | Adds its input to the response and ends the run. Use it for the final answer. |
| **Pane output** | Adds its input as a line in the run's pane (Sessions → the run's text button), newest first. Good for "what am I doing now" notes that shouldn't go in the reply. |

Every block and parameter is listed in [../loop-format.md](../loop-format.md).

## The editor

Open it from the main menu (**Loop editor**) or with **Edit loop** next to
an agent's loop.

- **Palette** (left): drag a block onto the canvas. **Inspector** (right):
  every setting of the selected block, with help text.
- Drag from an output to an input to link them; links only connect
  matching kinds (text, list, exchange).
- Problems (a missing link, a bad pattern) show as you work.
- **Test run** runs the loop with a message and lights up each block as it
  works; the run panel shows each block's output.
- **Make function**: select some blocks and they become one block, with the
  inputs and outputs you left open as its ports. Double-click to edit
  inside. Use this to keep a big loop readable.
- **Comments** on blocks, **labels** on links and **notes** on the canvas
  are saved with the loop, so it explains itself.
- Loops are saved in the library and can be exported and imported as JSON
  files. Start from a template: *Chummy assistant*, *Terminal assistant*,
  *Blank* or *Simple chat*.

To change a block's settings for **one agent** without touching the loop,
use Settings → Agents → your agent → **Agent Loop Configuration**.

## The Chummy assistant

The recommended loop. One model works with tools, call after call, until it
answers without one, like a coding agent. It says what it's doing, keeps
its own to-do list for bigger jobs, reads `CHUMMY.md` from the folder,
summarises the conversation when the context gets full, and is sent back
(twice at most) if it stops with steps unchecked. It works **one step at a
time**: before each step, *Decide what's next* picks the next step (the
first unchecked to-do), you see it in the run's pane, and the model doing
the work is told to do just that step, as the last thing it reads. This
keeps small models from wandering off. If a step fails, it says what went
wrong and tries again (3 times at most). (That's one small extra model call
per step; delete the *Decide what's next* block in the Agent loop, and
clear Work's Reminder, if you'd rather not.) Open it in the editor to
see how it's built; every part has comments.

## The Terminal assistant

The same loop, working only through the terminal, for models that are at
home in a shell. Its Work block has every tool switched off except **Run
command** (Chummy's own small shell, inside the sandbox), **Run shell**
(a real `sh`, only with the Container or OpenShell sandbox) and **Fetch
page**; your agent's own tool and internet switches still apply, so turn
on Run command for the agent. Its prompts list the shell's commands and
show how to do everyday jobs with them: read part of a file with
`sed -n`, search with `grep -rn`, write a file with a heredoc, edit with
`sed -i`, add things up with `awk`, run `python`. Each command it runs is
shown in the run's pane under the step it decided on. Run command has no
variables, loops or `$(…)`; the model is told to use pipes, `xargs`, `awk`
or a short Python script instead (`help` in the shell lists everything).

## Building loops that work

Small models get confused easily. What helps:

- **Keep each prompt to one job.** "Write a summary" and "Check the
  summary" are two model blocks, not one prompt asking for both.
- **Ask for a fixed format** you can test: "Answer YES or NO first", then a
  *Branch* on `(?i)^\s*yes`. Don't parse free-form prose.
- **Give each model block only the tools it needs.** A block can switch
  tools off; fewer tools means a shorter prompt and fewer wrong calls.
- **Check, then retry.** A cycle of *draft → check → Branch* (back to draft
  on NO) catches many mistakes. Keep the limits as a backstop.
- **Use memory for what every call should see** (the request, notes, a
  to-do list) instead of repeating it through many links.
- **Route failures.** Link *Failed* outputs somewhere useful (tell the
  model what went wrong, or reply with an explanation) instead of letting
  the run die. A model block's or Run tool's *Failed (exchange)* gives the
  conversation with the failure at its end: let the model explain it, take
  it back out with **Drop last messages**, and loop back to try again, with
  a limit (the Chummy assistant's *Report what went wrong* does this, 3
  times at most).
- **Lean beats heavy.** More scaffolding isn't always better: a simple
  agent loop often does better than elaborate planning with a mid-size
  model. Try the simple version first and add checks where it fails.
- **Read the agent logs.** When a loop misbehaves, open the run under the
  reply (or in the Logs window), show Block steps, and look at what each
  block actually received and sent.

## Loops for a specific job

For a fixed task (summarise every file dropped in a folder, a daily
report), build a loop that does just that task, step by step, with fixed
prompts, and run it from a **scheduled job** (main menu → Scheduler). A job
runs as an agent, with exactly that agent's folder, model and permissions.
See [../job-format.md](../job-format.md).
