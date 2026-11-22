/**
 * Built-in engine state for the Settings window: runtime install progress,
 * downloads in progress, engine status and installed models.
 */
import {
  commands,
  events,
  errorText,
  type EngineStatus,
  type LocalStatus,
  type Progress,
  type RuntimeStep,
} from "$lib/ipc";

interface DownloadState {
  progress: Progress | null;
  error: string | null;
  done: boolean;
}

class LocalStore {
  status = $state<LocalStatus | null>(null);
  engine = $state<EngineStatus>({ state: "stopped" });
  downloads = $state<Record<string, DownloadState>>({});
  install = $state<{ step: RuntimeStep | null; log: string[]; error: string | null; running: boolean }>({
    step: null,
    log: [],
    error: null,
    running: false,
  });
  /** Called when a download finishes and a connection was added. */
  onDownloaded: ((repo: string, providerId: string) => void) | null = null;
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    await this.refresh();
    await events.onEngine((e) => (this.engine = e));
    await events.onDownload((e) => {
      if (e.kind === "progress") {
        this.downloads[e.progress.repo] = { progress: e.progress, error: null, done: false };
        return;
      }
      const prev = this.downloads[e.repo]?.progress ?? null;
      if (e.kind === "done") {
        this.downloads[e.repo] = { progress: prev, error: null, done: true };
        this.onDownloaded?.(e.repo, e.provider_id);
      } else if (e.kind === "failed") {
        this.downloads[e.repo] = { progress: prev, error: errorText(e.error), done: false };
      } else {
        delete this.downloads[e.repo];
      }
      this.refresh();
    });
    await events.onRuntime((e) => {
      if (e.kind === "step") {
        this.install.running = true;
        this.install.step = e.step;
        if (e.step.line) {
          this.install.log = [...this.install.log.slice(-199), e.step.line];
        }
      } else {
        this.install.running = false;
        this.install.error = e.kind === "failed" ? errorText(e.error) : null;
        this.refresh();
      }
    });
  }

  async refresh() {
    try {
      const s = await commands.localStatus();
      this.status = s;
      this.engine = s.engine;
      this.install.running = s.runtime_installing || this.install.running;
      for (const repo of s.downloading) {
        this.downloads[repo] ??= { progress: null, error: null, done: false };
      }
    } catch (e) {
      console.error("[chummy] local status failed", errorText(e));
    }
  }

  async startInstall() {
    this.install = { step: null, log: [], error: null, running: false };
    const started = await commands.installRuntime();
    if (started) this.install.running = true;
    return started;
  }
}

export const local = new LocalStore();
