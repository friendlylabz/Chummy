/** The sandbox tools run in and its last check, kept in sync with Rust. */
import { commands, events, type SandboxStatus } from "$lib/ipc";

class SandboxStore {
  status = $state<SandboxStatus | null>(null);
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    this.status = await commands.sandboxStatus();
    await events.onSandboxChanged((s) => (this.status = s));
  }
}

export const sandbox = new SandboxStore();
