/**
 * First-run setup state. Chat stays disabled until Rust reports a model, a
 * loop and an agent (which has its own workspace) are all configured.
 */
import { commands, events, type SetupStatus } from "$lib/ipc";

class SetupStore {
  status = $state<SetupStatus>({
    model_configured: false,
    loop_configured: false,
    agent_configured: false,
  });
  loaded = $state(false);

  /** Chat needs a model, a loop and at least one agent. */
  get ready() {
    const s = this.status;
    return s.model_configured && s.loop_configured && s.agent_configured;
  }

  async init() {
    this.status = await commands.setupStatus();
    this.loaded = true;
    await events.onSetupChanged((s) => (this.status = s));
  }
}

export const setup = new SetupStore();
