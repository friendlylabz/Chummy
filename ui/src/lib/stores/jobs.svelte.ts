/** Scheduled jobs, kept in sync with Rust. */
import { commands, events, type Job, type JobsOverview, type RunRecord } from "$lib/ipc";

class JobsStore {
  overview = $state<JobsOverview>({ jobs: [], paused: false, bad_files: [] });
  /** Run history of the job being looked at, newest first. */
  history = $state<RunRecord[]>([]);
  historyFor = $state<string | null>(null);
  private started = false;

  async init() {
    if (this.started) return;
    this.started = true;
    await events.onJobsChanged((o) => {
      this.overview = o;
      if (this.historyFor) this.loadHistory(this.historyFor);
    });
    this.overview = await commands.listJobs();
  }

  view(id: string | null) {
    return this.overview.jobs.find((j) => j.job.id === id);
  }

  async loadHistory(id: string | null) {
    this.historyFor = id;
    if (!id) {
      this.history = [];
      return;
    }
    const h = await commands.jobHistory(id).catch(() => []);
    if (this.historyFor === id) this.history = h.reverse();
  }
}

export const jobs = new JobsStore();

/** A new job, ready to fill in. */
export function blankJob(agent: string): Job {
  return {
    format: "chummy.job",
    version: 1,
    id: "",
    name: "",
    enabled: true,
    agent,
    loop: null,
    message: "",
    file: null,
    triggers: [{ kind: "daily", time: "09:00", days: [] }],
    missed: "skip",
    time_limit_minutes: 0,
    save_reply_to: null,
    notify: "problems",
  };
}
