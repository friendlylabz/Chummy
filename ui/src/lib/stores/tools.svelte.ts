/** Installed tools and the tool runtime's setup progress, kept in sync with Rust. */
import { commands, events, type ToolsProgress, type ToolsStatus } from "$lib/ipc";

class ToolsStore {
  status = $state<ToolsStatus | null>(null);
  /** Latest progress while setting up; null when idle. */
  progress = $state<{ message: string; done: number; total: number } | null>(null);
  error = $state("");
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    this.status = await commands.toolsStatus();
    await events.onToolsChanged((s) => (this.status = s));
    await events.onToolsProgress((p: ToolsProgress) => {
      switch (p.type) {
        case "step":
          this.progress = { message: p.message, done: 0, total: 0 };
          this.error = "";
          break;
        case "finished":
          this.progress = null;
          break;
        case "failed":
          this.progress = null;
          this.error = p.error;
          break;
      }
    });
  }

  get installed() {
    return this.status?.tools ?? [];
  }

  get ready() {
    return !!this.status?.runtime && this.installed.length > 0;
  }
}

export const tools = new ToolsStore();
