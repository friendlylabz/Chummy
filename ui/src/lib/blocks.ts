/** How block categories look, shared by the editor and the logs. */
import type { Category } from "$lib/ipc";

export const categoryLabel: Record<Category, string> = {
  start: "Start",
  think: "Think",
  you: "You",
  exchange: "Exchange",
  text: "Text",
  flow: "Flow",
  workspace: "Workspace",
  memory: "Memory",
  finish: "Finish",
};

/** One hue per category, used for the dot on each block. */
export const categoryColor: Record<Category, string> = {
  start: "#6fb3f2",
  think: "var(--accent)",
  you: "#5ec8c8",
  exchange: "#f4a261",
  text: "#a78bfa",
  flow: "#e8c16d",
  workspace: "#7fd1a4",
  memory: "#c9a0dc",
  finish: "#ef8fa8",
};

