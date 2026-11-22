/**
 * First-run setup state. Chat stays disabled until Rust reports a workspace
 * folder, a model and an agent loop are all configured.
 */
import { commands, events, type SetupStatus } from "$lib/ipc";

class SetupStore {
  status = $state<SetupStatus>({
    workspace_configured: false,
    model_configured: false,
    loop_configured: false,
  });
  loaded = $state(false);

  get ready() {
    const s = this.status;
    return s.workspace_configured && s.model_configured && s.loop_configured;
  }

  async init() {
    this.status = await commands.setupStatus();
    this.loaded = true;
    await events.onSetupChanged((s) => (this.status = s));
  }
}

export const setup = new SetupStore();
