/** Words for the Review window. */
import type { Origin, ReviewBlock } from "$lib/ipc";

const ORIGINS: Record<Origin, string> = {
  you: "your message",
  history: "chat history",
  author: "the loop's own text",
  model: "a model",
  files: "your files",
  tools: "tool results",
  answers: "your answers",
  errors: "error messages",
};

export function originLabel(o: Origin): string {
  return ORIGINS[o];
}

/** Where an origin is worth noticing on its way out. */
export function originTone(o: Origin): "plain" | "watch" {
  return o === "files" || o === "history" || o === "answers" || o === "author" ? "watch" : "plain";
}

/** A block as people should see it: its name, always with its real type. */
export function blockName(b: ReviewBlock | undefined): string {
  if (!b) return "?";
  const name = b.label.trim() || b.id;
  const kind = name.toLowerCase() === b.kind_label.toLowerCase() ? b.kind_label : `“${name}” (${b.kind_label})`;
  return b.function ? `${kind} inside “${b.function}”` : kind;
}
