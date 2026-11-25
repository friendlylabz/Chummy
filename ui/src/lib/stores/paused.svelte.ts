/**
 * Paused chat runs. The list lives in Rust (the runs themselves are sealed
 * there, never sent to windows); this mirrors it and resumes or discards.
 */
import { commands, events, errorText, type PausedInfo } from "$lib/ipc";

class PausedStore {
  list = $state<PausedInfo[]>([]);
  /** Per chat: the last error from resuming or discarding its run. */
  errors = $state<Record<string, string>>({});
  /** Chats whose resume or discard is under way. */
  busy = $state<Record<string, boolean>>({});
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    await events.onPausedChanged((l) => (this.list = l));
    this.list = await commands.listPaused().catch(() => []);
  }

  forChat(agentId: string | null | undefined, chatId: string | null | undefined): PausedInfo | undefined {
    return this.list.find((p) => p.agent_id === agentId && p.chat_id === chatId);
  }

  async resume(p: PausedInfo): Promise<boolean> {
    return this.act(p, () => commands.resumeRun(p.agent_id, p.chat_id));
  }

  async discard(p: PausedInfo): Promise<boolean> {
    return this.act(p, () => commands.discardPaused(p.agent_id, p.chat_id));
  }

  private async act(p: PausedInfo, f: () => Promise<unknown>): Promise<boolean> {
    delete this.errors[p.chat_id];
    this.busy[p.chat_id] = true;
    try {
      await f();
      return true;
    } catch (e) {
      this.errors[p.chat_id] = errorText(e);
      return false;
    } finally {
      delete this.busy[p.chat_id];
    }
  }
}

export const paused = new PausedStore();
