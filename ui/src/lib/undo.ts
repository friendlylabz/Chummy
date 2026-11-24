// Undo and redo for a text box (Ctrl+Z / Cmd+Z, Ctrl+Shift+Z / Ctrl+Y).
// The app keeps its own history because the box's value is also set from
// code (cleared on send, an answer put in), which the browser's own undo
// doesn't follow, and desktop webviews don't always provide one.

export interface TextState {
  value: string;
  start: number;
  end: number;
}

/** Typing within this long of the last change joins the same undo step. */
export const GROUP_MS = 800;
/** Most undo steps kept. */
export const MAX_STEPS = 200;

export class TextUndo {
  private past: TextState[] = [];
  private future: TextState[] = [];
  private lastAt = 0;
  private lastKind = "";

  /**
   * Call before each change (the box's `beforeinput`), with the state the
   * change is about to replace. A typed word (with the space before it) is
   * one step; a pause, a paste or a deletion starts a new one.
   */
  before(current: TextState, kind: string, data: string | null, now = Date.now()) {
    // A space or new line starts a new step, which the next word joins.
    const typing = kind === "insertText" && !!data;
    const boundary = typing && /\s/.test(data);
    const joins =
      typing && !boundary && this.lastKind === "typing" && now - this.lastAt < GROUP_MS && this.past.length > 0;
    if (!joins) {
      const top = this.past.at(-1);
      if (!top || top.value !== current.value) this.past.push({ ...current });
      if (this.past.length > MAX_STEPS) this.past.shift();
    }
    this.future = [];
    this.lastKind = typing ? "typing" : kind;
    this.lastAt = now;
  }

  /** The state to go back to, or null; `current` becomes redoable. */
  undo(current: TextState): TextState | null {
    let prev = this.past.pop();
    while (prev && prev.value === current.value) prev = this.past.pop();
    if (!prev) return null;
    this.future.push({ ...current });
    this.lastKind = "";
    return prev;
  }

  /** The state to go forward to again, or null. */
  redo(current: TextState): TextState | null {
    const next = this.future.pop();
    if (!next) return null;
    this.past.push({ ...current });
    this.lastKind = "";
    return next;
  }

  /** Forgets everything (e.g. after the message is sent). */
  clear() {
    this.past = [];
    this.future = [];
    this.lastKind = "";
  }

  get canUndo() {
    return this.past.length > 0;
  }
}

/** Which history action a key press asks for, if any. */
export function undoKey(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">): "undo" | "redo" | null {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return null;
  const k = e.key.toLowerCase();
  if (k === "z") return e.shiftKey ? "redo" : "undo";
  if (k === "y" && !e.shiftKey) return "redo";
  return null;
}
