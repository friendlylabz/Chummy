/**
 * The active agent's chats: the list (newest first), the open chat and its
 * messages. Rust keeps the chats and which one is open; this follows its
 * `chats:changed` and `chat:message` events.
 */
import { commands, events, errorText, type ChatMessage, type ChatMeta } from "$lib/ipc";

/** How long Undo stays offered after the last delete. */
const UNDO_MS = 20_000;

class ChatStore {
  agentId = $state<string | null>(null);
  chatId = $state<string | null>(null);
  chats = $state<ChatMeta[]>([]);
  messages = $state<ChatMessage[]>([]);
  error = $state("");
  private started = false;
  /** Bumped on every switch, so a slow load can't overwrite a newer one. */
  private loading = 0;

  get current(): ChatMeta | null {
    return this.chats.find((c) => c.id === this.chatId) ?? null;
  }

  async init() {
    if (this.started) return;
    this.started = true;
    await events.onChatMessage((e) => {
      if (e.agent_id !== this.agentId || e.chat_id !== this.chatId) return;
      if (!this.messages.some((m) => m.id === e.message.id)) this.messages.push(e.message);
    });
    await events.onChatMessageDeleted((e) => {
      if (e.agent_id !== this.agentId || e.chat_id !== this.chatId) return;
      this.messages = this.messages.filter((m) => m.id !== e.message_id);
    });
    await events.onChatMessageRestored((e) => {
      if (e.agent_id !== this.agentId || e.chat_id !== this.chatId) return;
      if (this.messages.some((m) => m.id === e.message.id)) return;
      const next = [...this.messages];
      next.splice(Math.min(e.position, next.length), 0, e.message);
      this.messages = next;
    });
    await events.onChatsChanged((e) => {
      if (e.agent_id !== this.agentId) return;
      this.chats = e.index.chats;
      if (e.index.active && e.index.active !== this.chatId) this.open(e.index.active);
    });
  }

  /** Shows an agent's open chat (making its first one if it has none). */
  async show(agentId: string | null) {
    if (agentId === this.agentId) return;
    this.agentId = agentId;
    this.chatId = null;
    this.chats = [];
    this.messages = [];
    this.error = "";
    if (!agentId) return;
    const n = ++this.loading;
    const ix = await commands.listChats(agentId).catch(() => null);
    if (n !== this.loading || !ix) return;
    this.chats = ix.chats;
    if (ix.active) await this.open(ix.active);
  }

  private async open(chatId: string) {
    const agentId = this.agentId;
    if (!agentId) return;
    if (chatId !== this.chatId) this.forgetUndo();
    this.chatId = chatId;
    this.error = "";
    const n = ++this.loading;
    const messages = await commands.getChat(agentId, chatId).catch(() => []);
    if (n === this.loading) this.messages = messages;
  }

  /** Opens another of the agent's chats (and remembers it). */
  async choose(chatId: string) {
    if (!this.agentId || chatId === this.chatId) return;
    await this.open(chatId);
    await commands.setActiveChat(this.agentId, chatId).catch((e) => (this.error = errorText(e)));
  }

  /** Starts a new chat and opens it. */
  async create() {
    if (!this.agentId) return;
    try {
      const c = await commands.newChat(this.agentId);
      if (!this.chats.some((x) => x.id === c.id)) this.chats = [c, ...this.chats];
      await this.open(c.id);
    } catch (e) {
      this.error = errorText(e);
    }
  }

  async remove(chatId: string) {
    if (!this.agentId) return;
    try {
      await commands.deleteChat(this.agentId, chatId);
    } catch (e) {
      this.error = errorText(e);
    }
  }

  /** Messages deleted from the open chat that Undo can bring back, newest
   *  last. Cleared when another chat opens, a message is sent, or after a
   *  while (`UNDO_MS` after the last delete). */
  undoable = $state<string[]>([]);
  private undoTimer: ReturnType<typeof setTimeout> | undefined;

  private forgetUndo() {
    clearTimeout(this.undoTimer);
    this.undoable = [];
  }

  /** Deletes a message from the open chat (and from what the agent sees). */
  async deleteMessage(id: string) {
    if (!this.agentId || !this.chatId) return;
    this.error = "";
    try {
      await commands.deleteChatMessage(this.agentId, this.chatId, id);
      this.undoable = [...this.undoable, id];
      clearTimeout(this.undoTimer);
      this.undoTimer = setTimeout(() => this.forgetUndo(), UNDO_MS);
    } catch (e) {
      this.error = errorText(e);
    }
  }

  /** Puts the most recently deleted message back where it was. */
  async undoDelete() {
    const id = this.undoable.at(-1);
    if (!id || !this.agentId || !this.chatId) return;
    this.error = "";
    try {
      await commands.restoreChatMessage(this.agentId, this.chatId, id);
      this.undoable = this.undoable.slice(0, -1);
      if (!this.undoable.length) this.forgetUndo();
    } catch (e) {
      this.error = errorText(e);
    }
  }

  dismissUndo() {
    this.forgetUndo();
  }

  async send(text: string) {
    if (!this.agentId || !this.chatId) return false;
    this.forgetUndo();
    this.error = "";
    try {
      await commands.sendMessage(this.agentId, this.chatId, text);
      return true;
    } catch (e) {
      this.error = errorText(e);
      return false;
    }
  }
}

export const chat = new ChatStore();
