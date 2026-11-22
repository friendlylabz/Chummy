//! # chummy-log
//!
//! Two kinds of log, both JSON lines in the Chummy folder's `logs/`:
//!
//! - [`system`]: what the app itself did (starts, settings changes, runs,
//!   jobs, tools, errors), one file per day in `logs/system/`.
//! - [`runs`]: one file per run of a loop in `logs/runs/<day>/`, recording
//!   every block it executed with its inputs and outputs, every model call,
//!   tool call, question and reply, and how it ended.
//!
//! Nothing here depends on Tauri, so chummyd can use it as is.

pub mod runs;
pub mod system;

/// The local date, `YYYY-MM-DD`, for a Unix time in milliseconds.
pub fn day_of(ms: u64) -> String {
    jiff::Timestamp::from_millisecond(ms as i64)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| "1970-01-01".into())
}

/// Whether `s` is a day as used for log file names.
pub fn valid_day(s: &str) -> bool {
    s.len() == 10
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

pub use chummy_flow::log::now_ms;
