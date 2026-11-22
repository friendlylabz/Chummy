/**
 * Loop runs in progress, shared by the chat window and the editor. Rust
 * streams `run:*` events; this keeps per-run block states and live text.
 */
import { commands, events, type BlockState, type LogEntry, type RunInfo, type RunOutcome } from "$lib/ipc";
import { entryOf } from "$lib/logs";

export interface RunView {
  info: RunInfo;
  /** Latest state of every block that has done anything. */
  blocks: Record<string, BlockState>;
  /** Block currently running, if any. */
  current: string | null;
  /** Its label. */
  currentLabel: string;
  /** Text streamed by the current model call. */
  streaming: string;
  /** Short previews of each block's last output. */
  outputs: Record<string, string>;
  replies: string[];
  /** Tool calls, their results and notices, in order. */
  activity: { kind: "call" | "result" | "error" | "notice"; block: string; text: string }[];
  outcome: RunOutcome | null;
  /** A block is waiting for the user. */
  waitingForYou: boolean;
  /** What the run's log has recorded so far (steps, model calls, tools…). */
  entries: LogEntry[];
}

class RunsStore {
  runs = $state<Record<string, RunView>>({});
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    for (const info of await commands.listRuns()) this.add(info);
    await events.onRunStarted((info) => this.add(info));
    await events.onRunEvent(({ run_id, event }) => {
      const r = this.runs[run_id];
      if (!r) return;
      const entry = entryOf(event);
      if (entry) r.entries.push(entry);
      switch (event.type) {
        case "block_state":
          r.blocks[event.block] = event.state;
          if (event.state === "running") {
            r.current = event.block;
            r.currentLabel = event.label;
            r.streaming = "";
          } else if (r.current === event.block) {
            r.current = null;
          }
          break;
        case "llm_text":
          r.streaming += event.text;
          break;
        case "block_output":
          r.outputs[event.block] = event.preview;
          break;
        case "reply":
          r.replies.push(event.text);
          break;
        case "tool_call":
          r.activity.push({ kind: "call", block: event.block, text: `${event.tool} ${event.arguments}` });
          break;
        case "tool_result":
          r.activity.push({ kind: event.is_error ? "error" : "result", block: event.block, text: `${event.tool}: ${event.preview}` });
          break;
        case "notice":
          r.activity.push({ kind: "notice", block: event.block, text: event.text });
          break;
        case "asked":
          r.waitingForYou = true;
          r.activity.push({ kind: "notice", block: event.block, text: "Waiting for your answer…" });
          break;
        case "answered":
          r.waitingForYou = false;
          break;
      }
    });
    await events.onRunFinished(({ run_id, outcome }) => {
      const r = this.runs[run_id];
      if (!r) return;
      r.outcome = outcome;
      r.current = null;
      // Keep finished runs briefly so views can show the result.
      setTimeout(() => delete this.runs[run_id], r.info.source === "test" ? 600_000 : 1500);
    });
  }

  private add(info: RunInfo) {
    this.runs[info.run_id] = {
      info,
      blocks: {},
      current: null,
      currentLabel: "",
      streaming: "",
      outputs: {},
      replies: [],
      activity: [],
      outcome: null,
      waitingForYou: false,
      entries: [],
    };
  }

  get active(): RunView[] {
    return Object.values(this.runs).filter((r) => !r.outcome);
  }

  /** A scheduled job running as this agent, if any. */
  jobForAgent(agentId: string | null): RunView | undefined {
    return Object.values(this.runs).find((r) => r.info.agent_id === agentId && r.info.source === "job" && !r.outcome);
  }

  forAgent(agentId: string | null): RunView | undefined {
    return Object.values(this.runs).find((r) => r.info.agent_id === agentId && r.info.source === "chat" && !r.outcome);
  }
}

export const runs = new RunsStore();
