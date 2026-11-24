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
  /** When the run started, in Unix ms (for the live timer). */
  startedMs: number;
  /** What its Pane output blocks have shown, newest first (at most PANE_LINES). */
  pane: string[];
}

/** Lines a run's pane keeps (as in Rust). */
export const PANE_LINES = 500;

/** A block that started (and maybe finished), queued for the Sessions panel. */
interface StatusUpdate {
  run: string;
  block: string;
  label: string;
  start: number;
  end?: number;
}

class RunsStore {
  runs = $state<Record<string, RunView>>({});
  private started = false;
  /** Status updates for the Sessions panel. Not reactive on purpose: block
   *  events only add here, and the panel reads it on its own clock. */
  private statusQueue: StatusUpdate[] = [];
  /** What the panel was last told for each run. */
  private shown = new Map<string, string>();

  async init() {
    if (this.started) return;
    this.started = true;
    // Runs already going when this window opened: their start is known to
    // the second; new ones are timed from the moment they're announced.
    for (const info of await commands.listRuns()) this.add(info, info.started_at * 1000);
    await events.onRunStarted((info) => this.add(info, Date.now()));
    await events.onRunEvent(({ run_id, event }) => {
      const r = this.runs[run_id];
      if (!r) return;
      const entry = entryOf(event);
      if (entry) r.entries.push(entry);
      switch (event.type) {
        case "block_state":
          r.blocks[event.block] = event.state;
          this.queueStatus(run_id, event.block, event.state === "running" ? event.label : null);
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
        case "pane":
          r.pane.unshift(event.text);
          if (r.pane.length > PANE_LINES) r.pane.length = PANE_LINES;
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

  private add(info: RunInfo, startedMs: number) {
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
      startedMs,
      pane: [],
    };
  }

  private queueStatus(run: string, block: string, label: string | null) {
    const now = performance.now();
    if (label !== null) {
      this.statusQueue.push({ run, block, label, start: now });
      return;
    }
    for (let i = this.statusQueue.length - 1; i >= 0; i--) {
      const u = this.statusQueue[i];
      if (u.run === run && u.block === block && u.end === undefined) {
        u.end = now;
        break;
      }
    }
  }

  /**
   * Reads the queued status updates (the Sessions panel calls this every
   * half second): for each working run, the step that ran longest in the
   * last `windowMs`. Blocks that come and go in a moment never show, and a
   * run between steps keeps its last status, so the panel stays calm.
   */
  takeStatus(windowMs = 500): Map<string, string> {
    const now = performance.now();
    const from = now - windowMs;
    const best = new Map<string, { label: string; time: number }>();
    for (const u of this.statusQueue) {
      const running = u.end === undefined;
      const time = Math.min(u.end ?? now, now) - Math.max(u.start, from);
      if (!running && time < 50) continue;
      const b = best.get(u.run);
      if (!b || time >= b.time) best.set(u.run, { label: u.label, time });
    }
    this.statusQueue = this.statusQueue.filter((u) => (u.end === undefined || u.end > from) && this.runs[u.run]);
    const out = new Map<string, string>();
    for (const r of this.active) {
      const id = r.info.run_id;
      const label = r.waitingForYou ? "Waiting for you" : (best.get(id)?.label ?? this.shown.get(id) ?? "Working");
      out.set(id, label);
    }
    this.shown = new Map([...out].filter(([, l]) => l !== "Waiting for you"));
    return out;
  }

  /**
   * Fills in a run's pane from Rust's copy (`run_pane`), for a window that
   * opened after the run had already shown lines. Rust's copy always has
   * at least what this window got from events.
   */
  seedPane(runId: string, lines: string[]) {
    const r = this.runs[runId];
    if (r && lines.length > r.pane.length) r.pane = lines;
  }

  get active(): RunView[] {
    return Object.values(this.runs).filter((r) => !r.outcome);
  }

  /** A scheduled job running as this agent, if any. */
  jobForAgent(agentId: string | null): RunView | undefined {
    return Object.values(this.runs).find((r) => r.info.agent_id === agentId && r.info.source === "job" && !r.outcome);
  }

  /** A chat of this agent that's working, if any. */
  forAgent(agentId: string | null): RunView | undefined {
    return Object.values(this.runs).find((r) => r.info.agent_id === agentId && r.info.source === "chat" && !r.outcome);
  }

  /** The run of this chat, if it's working. */
  forChat(chatId: string | null): RunView | undefined {
    if (!chatId) return undefined;
    return Object.values(this.runs).find((r) => r.info.chat_id === chatId && !r.outcome);
  }
}

export const runs = new RunsStore();
