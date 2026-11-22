/**
 * First-run setup state. Chat stays disabled until Rust reports that both a
 * model and an agent loop are configured.
 */
import { commands, events, type SetupStatus } from "$lib/ipc";

class SetupStore {
  status = $state<SetupStatus>({ model_configured: false, loop_configured: false });
  loaded = $state(false);

  get ready() {
    return this.status.model_configured && this.status.loop_configured;
  }

  async init() {
    this.status = await commands.setupStatus();
    this.loaded = true;
    await events.onSetupChanged((s) => (this.status = s));
  }
}

export const setup = new SetupStore();
