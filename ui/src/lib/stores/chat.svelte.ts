/** The chat with the active agent. */
import { commands, events, errorText, type ChatMessage } from "$lib/ipc";

class ChatStore {
  agentId = $state<string | null>(null);
  messages = $state<ChatMessage[]>([]);
  error = $state("");
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    await events.onChatMessage((e) => {
      if (e.agent_id !== this.agentId) return;
      if (e.cleared) this.messages = [];
      else if (e.message && !this.messages.some((m) => m.id === e.message!.id)) this.messages.push(e.message);
    });
  }

  async show(agentId: string | null) {
    if (agentId === this.agentId) return;
    this.agentId = agentId;
    this.error = "";
    this.messages = agentId ? await commands.getChat(agentId).catch(() => []) : [];
  }

  async send(text: string) {
    if (!this.agentId) return false;
    this.error = "";
    try {
      await commands.sendMessage(this.agentId, text);
      return true;
    } catch (e) {
      this.error = errorText(e);
      return false;
    }
  }
}

export const chat = new ChatStore();
