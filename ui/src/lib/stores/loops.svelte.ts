/** The block catalog and the loop library, kept in sync with Rust. */
import { commands, events, type BlockInfo, type LoopSummary } from "$lib/ipc";

class LoopsStore {
  catalog = $state<BlockInfo[]>([]);
  list = $state<LoopSummary[]>([]);
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    const [catalog, list] = await Promise.all([commands.blockCatalog().catch(() => []), commands.listLoops()]);
    this.catalog = catalog;
    this.list = list;
    await events.onLoopsChanged((l) => (this.list = l));
  }

  info(kind: string): BlockInfo | undefined {
    return this.catalog.find((b) => b.kind === kind);
  }

  name(id: string): string | null {
    return this.list.find((l) => l.id === id)?.name ?? null;
  }
}

export const loops = new LoopsStore();
