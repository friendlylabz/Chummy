//! When a job runs.
//!
//! Times are wall-clock times in the computer's time zone (for chummyd, the
//! zone in its config). Daylight saving is handled the usual way: a time
//! that doesn't exist on the day the clocks go forward runs just after the
//! jump, and a time that happens twice when they go back runs the first
//! time.

use jiff::civil::{DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use serde::{Deserialize, Serialize};

/// Shortest gap between runs of a repeating schedule.
pub const MIN_EVERY_MINUTES: u32 = 5;
/// Longest gap: four weeks.
pub const MAX_EVERY_MINUTES: u32 = 40_320;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    pub const ALL: [Day; 7] = [
        Day::Mon,
        Day::Tue,
        Day::Wed,
        Day::Thu,
        Day::Fri,
        Day::Sat,
        Day::Sun,
    ];

    fn weekday(self) -> Weekday {
        match self {
            Day::Mon => Weekday::Monday,
            Day::Tue => Weekday::Tuesday,
            Day::Wed => Weekday::Wednesday,
            Day::Thu => Weekday::Thursday,
            Day::Fri => Weekday::Friday,
            Day::Sat => Weekday::Saturday,
            Day::Sun => Weekday::Sunday,
        }
    }

    fn short(self) -> &'static str {
        match self {
            Day::Mon => "Mon",
            Day::Tue => "Tue",
            Day::Wed => "Wed",
            Day::Thu => "Thu",
            Day::Fri => "Fri",
            Day::Sat => "Sat",
            Day::Sun => "Sun",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// Once, at a local date and time like `2026-12-01T09:00`.
    Once { at: String },
    /// Repeatedly, this many minutes apart, counted from when the job was
    /// switched on.
    Every { minutes: u32 },
    /// At a local time (`HH:MM`, 24-hour) on the chosen days (none: every day).
    Daily {
        time: String,
        #[serde(default)]
        days: Vec<Day>,
    },
    /// When a file in this folder of the agent's (not its subfolders) is
    /// added or changed. Each file starts its own run.
    Folder { path: String },
}

fn parse_time(s: &str) -> Option<Time> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    Time::new(h.parse().ok()?, m.parse().ok()?, 0, 0).ok()
}

fn parse_at(s: &str) -> Option<DateTime> {
    s.trim().parse::<DateTime>().ok()
}

impl Trigger {
    /// What's wrong with it, in words for the user.
    pub fn problem(&self) -> Option<String> {
        match self {
            Trigger::Once { at } if parse_at(at).is_none() => Some(format!(
                "The date and time “{at}” isn't valid; use a form like 2026-12-01T09:00."
            )),
            Trigger::Every { minutes } if *minutes < MIN_EVERY_MINUTES => Some(format!(
                "Repeating schedules must be at least {MIN_EVERY_MINUTES} minutes apart."
            )),
            Trigger::Every { minutes } if *minutes > MAX_EVERY_MINUTES => {
                Some("Repeating schedules can be four weeks apart at most.".into())
            }
            Trigger::Daily { time, .. } if parse_time(time).is_none() => Some(format!(
                "The time “{time}” isn't valid; use 24-hour time like 08:30."
            )),
            Trigger::Folder { path } => {
                crate::job::path_problem(path).map(|why| format!("The watched folder {why}."))
            }
            _ => None,
        }
    }

    /// Whether this trigger fires on a schedule (rather than on file changes).
    pub fn is_timed(&self) -> bool {
        !matches!(self, Trigger::Folder { .. })
    }

    /// The first time this trigger fires strictly after `after`. `anchor`
    /// is when repeating schedules count from.
    pub fn next_after(
        &self,
        after: Timestamp,
        anchor: Timestamp,
        tz: &TimeZone,
    ) -> Option<Timestamp> {
        match self {
            Trigger::Folder { .. } => None,
            Trigger::Once { at } => {
                let t = parse_at(at)?
                    .to_zoned_datetime(tz.clone())
                    .ok()?
                    .timestamp();
                (t > after).then_some(t)
            }
            Trigger::Every { minutes } => {
                let step = i64::from((*minutes).max(1)) * 60;
                let a = anchor.as_second();
                let now = after.as_second();
                let k = if now < a { 1 } else { (now - a) / step + 1 };
                Timestamp::from_second(a.checked_add(k.checked_mul(step)?)?).ok()
            }
            Trigger::Daily { time, days } => {
                let time = parse_time(time)?;
                let mut date = after.to_zoned_datetime(tz.clone()).date();
                // Today and the next week cover every choice of days.
                for _ in 0..9 {
                    let wd = date.weekday();
                    if days.is_empty() || days.iter().any(|d| d.weekday() == wd) {
                        let t = date
                            .to_datetime(time)
                            .to_zoned_datetime(tz.clone())
                            .ok()?
                            .timestamp();
                        if t > after {
                            return Some(t);
                        }
                    }
                    date = date.tomorrow().ok()?;
                }
                None
            }
        }
    }

    /// A short description, e.g. "Weekdays at 09:00".
    pub fn describe(&self) -> String {
        match self {
            Trigger::Once { at } => match parse_at(at) {
                Some(dt) => format!("Once, on {} at {}", dt.date(), dt.strftime("%H:%M")),
                None => format!("Once, at {at}"),
            },
            Trigger::Every { minutes } => {
                let m = *minutes;
                let (n, unit) = if m % 1440 == 0 {
                    (m / 1440, "day")
                } else if m % 60 == 0 {
                    (m / 60, "hour")
                } else {
                    (m, "minute")
                };
                if n == 1 {
                    format!("Every {unit}")
                } else {
                    format!("Every {n} {unit}s")
                }
            }
            Trigger::Daily { time, days } => {
                let mut set: Vec<Day> = Day::ALL.into_iter().filter(|d| days.contains(d)).collect();
                if set.is_empty() {
                    set = Day::ALL.to_vec();
                }
                let when = match set.as_slice() {
                    s if s.len() == 7 => "Every day".to_string(),
                    [Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri] => "Weekdays".into(),
                    [Day::Sat, Day::Sun] => "Weekends".into(),
                    s => s.iter().map(|d| d.short()).collect::<Vec<_>>().join(", "),
                };
                format!("{when} at {}", time.trim())
            }
            Trigger::Folder { path } => {
                let p = path.trim().trim_end_matches(['/', '\\']);
                if p.is_empty() || p == "." {
                    "When files change in the agent's folder".into()
                } else {
                    format!("When files change in {p}/")
                }
            }
        }
    }
}

/// The current time in `tz`.
pub fn zoned(ts: Timestamp, tz: &TimeZone) -> ZonedDateTime {
    ts.to_zoned_datetime(tz.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ny() -> TimeZone {
        TimeZone::get("America/New_York").unwrap()
    }

    fn at(s: &str) -> Timestamp {
        s.parse::<DateTime>()
            .unwrap()
            .to_zoned_datetime(ny())
            .unwrap()
            .timestamp()
    }

    fn local(t: Timestamp) -> String {
        t.to_zoned_datetime(ny())
            .strftime("%a %Y-%m-%d %H:%M")
            .to_string()
    }

    #[test]
    fn daily_picks_the_next_matching_day() {
        let t = Trigger::Daily {
            time: "09:00".into(),
            days: vec![Day::Mon, Day::Wed],
        };
        // Sunday afternoon → Monday 09:00.
        let n = t.next_after(at("2026-11-22T13:28"), at("2026-01-01T00:00"), &ny());
        assert_eq!(local(n.unwrap()), "Mon 2026-11-23 09:00");
        // Exactly at 09:00 Monday → Wednesday.
        let n = t.next_after(at("2026-11-23T09:00"), at("2026-01-01T00:00"), &ny());
        assert_eq!(local(n.unwrap()), "Wed 2026-11-25 09:00");
        // Every day, later today.
        let every = Trigger::Daily {
            time: "18:05".into(),
            days: vec![],
        };
        let n = every.next_after(at("2026-11-22T13:28"), at("2026-01-01T00:00"), &ny());
        assert_eq!(local(n.unwrap()), "Sun 2026-11-22 18:05");
    }

    #[test]
    fn daylight_saving_gaps_and_folds() {
        // 2026-03-08: 02:30 doesn't exist in New York; it runs at 03:30.
        let t = Trigger::Daily {
            time: "02:30".into(),
            days: vec![],
        };
        let n = t
            .next_after(at("2026-03-08T00:00"), at("2026-01-01T00:00"), &ny())
            .unwrap();
        assert_eq!(local(n), "Sun 2026-03-08 03:30");
        // 2026-11-01: 01:30 happens twice; only the first runs.
        let t = Trigger::Daily {
            time: "01:30".into(),
            days: vec![],
        };
        let first = t
            .next_after(at("2026-11-01T00:00"), at("2026-01-01T00:00"), &ny())
            .unwrap();
        let next = t.next_after(first, at("2026-01-01T00:00"), &ny()).unwrap();
        assert_eq!(local(next), "Mon 2026-11-02 01:30");
        assert_eq!(next.as_second() - first.as_second(), 25 * 3600);
    }

    #[test]
    fn every_counts_from_the_anchor() {
        let t = Trigger::Every { minutes: 90 };
        let anchor = at("2026-11-22T08:00");
        let n = t.next_after(at("2026-11-22T13:28"), anchor, &ny()).unwrap();
        assert_eq!(local(n), "Sun 2026-11-22 14:00");
        let n = t.next_after(at("2026-11-22T14:00"), anchor, &ny()).unwrap();
        assert_eq!(local(n), "Sun 2026-11-22 15:30");
        // Before the anchor: the first run is one step after it.
        let n = t.next_after(at("2026-11-21T00:00"), anchor, &ny()).unwrap();
        assert_eq!(local(n), "Sun 2026-11-22 09:30");
    }

    #[test]
    fn describes_and_checks() {
        let d = |days: Vec<Day>| {
            Trigger::Daily {
                time: "09:00".into(),
                days,
            }
            .describe()
        };
        assert_eq!(d(vec![]), "Every day at 09:00");
        assert_eq!(
            d(vec![Day::Fri, Day::Thu, Day::Wed, Day::Tue, Day::Mon]),
            "Weekdays at 09:00"
        );
        assert_eq!(d(vec![Day::Sun, Day::Sat]), "Weekends at 09:00");
        assert_eq!(d(vec![Day::Wed, Day::Mon]), "Mon, Wed at 09:00");
        assert_eq!(Trigger::Every { minutes: 120 }.describe(), "Every 2 hours");
        assert_eq!(Trigger::Every { minutes: 1440 }.describe(), "Every day");
        assert_eq!(
            Trigger::Every { minutes: 45 }.describe(),
            "Every 45 minutes"
        );
        assert_eq!(
            Trigger::Folder { path: ".".into() }.describe(),
            "When files change in the agent's folder"
        );
        for bad in ["9", "25:00", "09:60", "9:5", "nine"] {
            let t = Trigger::Daily {
                time: bad.into(),
                days: vec![],
            };
            assert!(t.problem().is_some(), "{bad}");
        }
        assert!(
            Trigger::Daily {
                time: "9:05".into(),
                days: vec![]
            }
            .problem()
            .is_none()
        );
        assert!(
            Trigger::Once {
                at: "tomorrow".into()
            }
            .problem()
            .is_some()
        );
        let typo: Result<Trigger, _> =
            serde_json::from_str(r#"{ "kind": "every", "minutes": 10, "minuts": 5 }"#);
        assert!(typo.is_err());

        // (was once_and_folder)
        {
            let t = Trigger::Once {
                at: "2026-12-01T09:00".into(),
            };
            let a = at("2026-01-01T00:00");
            assert!(t.next_after(at("2026-11-22T00:00"), a, &ny()).is_some());
            assert!(t.next_after(at("2026-12-01T09:00"), a, &ny()).is_none());
            let f = Trigger::Folder {
                path: "inbox".into(),
            };
            assert!(f.next_after(a, a, &ny()).is_none() && !f.is_timed());
        }
    }
}
