//! # chummy-scheduler
//!
//! Jobs: one agent's loop, started with a message, on a schedule or when
//! files change in the agent's folder. Plus the one way any run asks the
//! user something ([`asks`]).
//!
//! This crate has no app or window code, so the same jobs and the same
//! manager run inside the Chummy app today and inside chummyd (the planned
//! background service) later:
//!
//! - [`job`]: the `chummy.job` file format.
//! - [`trigger`]: schedules and when they next fire.
//! - [`plan`]: what runs when (queue, one run per job and per agent, missed
//!   times), as a plain state machine.
//! - [`watch`]: folder triggers, by polling through the agent's jail.
//! - [`store`]: jobs, their state and history on disk.
//! - [`manager`]: drives it all through a [`manager::JobRunner`], which is
//!   the only part each host writes.
//! - [`asks`]: questions and tool approvals waiting for the user.
//!
//! A job never grants anything: it names an agent, and runs with exactly
//! that agent's folder, model and permissions.

pub mod asks;
pub mod job;
pub mod manager;
pub mod plan;
pub mod store;
pub mod trigger;
pub mod watch;

pub use asks::{AnswerError, AskBroker, AskEvent, Origin, Pending, RunKind};
pub use job::{Job, JobError, Missed, Notify};
pub use manager::{JobRunner, JobView, Manager, Overview, RunRequest, RunResult};
pub use plan::{Cause, JobState, Planner};
pub use store::{BadJobFile, JobStore, RecordStatus, RunRecord};
pub use trigger::{Day, Trigger};
pub use watch::{FileStamp, Snapshot};
