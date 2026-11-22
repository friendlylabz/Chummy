/**
 * Questions and tool approvals waiting for the user, from every run. The
 * list lives in Rust; this mirrors it and sends answers back.
 */
import { commands, events, errorText, type Answer, type Pending } from "$lib/ipc";

class AsksStore {
  pending = $state<Pending[]>([]);
  /** Per question: the last error from answering it. */
  errors = $state<Record<string, string>>({});
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    await events.onAsksChanged((p) => (this.pending = p));
    this.pending = await commands.listAsks().catch(() => []);
  }

  /** Questions to show in an agent's chat: its chats and its jobs. */
  forAgent(agentId: string | null): Pending[] {
    return this.pending.filter((p) => p.origin.agent_id === agentId && p.origin.kind !== "test");
  }

  forRun(runId: string | null | undefined): Pending[] {
    return this.pending.filter((p) => p.origin.run_id === runId);
  }

  forJob(jobId: string): Pending[] {
    return this.pending.filter((p) => p.origin.job_id === jobId);
  }

  async answer(id: string, answer: Answer): Promise<boolean> {
    delete this.errors[id];
    try {
      await commands.answerAsk(id, answer);
      return true;
    } catch (e) {
      this.errors[id] = errorText(e);
      return false;
    }
  }
}

export const asks = new AsksStore();
