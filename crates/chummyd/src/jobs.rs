//! Jobs from the bundle, run through `chummy-session` exactly as the app
//! runs them. Their state and history are saved with the sealed state.

use chummy_scheduler::{Job, JobRunner, RecordStatus, RunRecord, RunRequest, RunResult, Snapshot};
use chummy_session::Platform;

use crate::platform::DaemonPlatform;
use crate::router::Input;

pub struct DaemonJobs {
    pub p: DaemonPlatform,
}

impl JobRunner for DaemonJobs {
    /// Jobs run alongside the agents' conversations (only a test run would
    /// hold them back, and chummyd has none).
    fn agent_busy(&self, agent: &str) -> bool {
        !self.p.session().job_may_start(agent)
    }

    async fn run(&self, req: RunRequest) -> RunResult {
        chummy_session::run_job(&self.p, &req).await
    }

    fn snapshot(&self, agent: &str, folder: &str) -> Result<Snapshot, String> {
        chummy_session::snapshot(&self.p, agent, folder)
    }

    fn changed(&self, _job: Option<&str>) {
        self.p.mark_dirty();
    }

    fn finished(&self, job: &Job, record: &RunRecord) {
        let problem = !matches!(record.status, RecordStatus::Completed);
        chummy_log::system::log(
            if problem {
                chummy_log::system::Level::Warn
            } else {
                chummy_log::system::Level::Info
            },
            "jobs",
            format!(
                "Job {} finished: {}",
                job.id,
                format!("{:?}", record.status).to_lowercase()
            ),
            &[("run", &record.run_id)],
        );
        self.p.mark_dirty();
        let _ = self
            .p
            .0
            .inbox
            .send(Input::JobFinished(Box::new((job.clone(), record.clone()))));
    }
}
