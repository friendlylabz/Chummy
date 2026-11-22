//! Tokens used per model connection per day, for daily limits.
//!
//! Days are local calendar days. Each connection's allowance for a day is
//! its daily limit plus whatever the user allowed on top that day. Counts
//! come from what the server reports, or are estimated (about 4 characters a
//! token) when it doesn't say.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use chummy_store::JsonDoc;
use jiff::Zoned;
use serde::{Deserialize, Serialize};

/// Days of history kept.
const KEEP_DAYS: usize = 31;

/// One connection's use on one day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DayUse {
    /// Tokens sent and received.
    pub used: u64,
    /// More allowed by the user on top of the limit, that day.
    pub extra: u64,
    pub calls: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct UsageDoc {
    version: u32,
    /// Day (`YYYY-MM-DD`) → connection id → use.
    days: BTreeMap<String, BTreeMap<String, DayUse>>,
}

pub struct UsageLedger {
    doc: Option<JsonDoc<UsageDoc>>,
    inner: Mutex<UsageDoc>,
}

fn today() -> String {
    Zoned::now().strftime("%Y-%m-%d").to_string()
}

/// Time until the next local midnight, when allowances refresh.
pub fn until_midnight() -> Duration {
    let now = Zoned::now();
    let next = now
        .date()
        .tomorrow()
        .ok()
        .and_then(|d| d.to_zoned(now.time_zone().clone()).ok());
    match next {
        Some(n) => {
            let secs = n.timestamp().as_second() - now.timestamp().as_second();
            Duration::from_secs(secs.max(1) as u64)
        }
        None => Duration::from_secs(3600),
    }
}

/// A rough token count for text, when the server doesn't report one.
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

impl UsageLedger {
    /// Keeps the ledger in `path` (created when first written).
    pub fn open(path: &Path) -> Self {
        let doc = JsonDoc::<UsageDoc>::new(path);
        let inner = doc.load(|v| v).map(|l| l.value).unwrap_or_default();
        Self {
            doc: Some(doc),
            inner: Mutex::new(inner),
        }
    }

    /// A ledger that isn't saved (tests, test runs).
    pub fn in_memory() -> Self {
        Self {
            doc: None,
            inner: Mutex::default(),
        }
    }

    fn with_today(&self, provider: &str, f: impl FnOnce(&mut DayUse)) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let day = today();
        f(g.days
            .entry(day)
            .or_default()
            .entry(provider.to_string())
            .or_default());
        while g.days.len() > KEEP_DAYS {
            let oldest = g.days.keys().next().cloned();
            if let Some(o) = oldest {
                g.days.remove(&o);
            }
        }
        if let Some(doc) = &self.doc {
            g.version = 1;
            let _ = doc.save(&g);
        }
    }

    /// Records tokens used by `provider`.
    pub fn add(&self, provider: &str, tokens: u64) {
        self.with_today(provider, |d| {
            d.used = d.used.saturating_add(tokens);
            d.calls = d.calls.saturating_add(1);
        });
    }

    /// Allows `tokens` more for `provider` today.
    pub fn grant(&self, provider: &str, tokens: u64) {
        self.with_today(provider, |d| d.extra = d.extra.saturating_add(tokens));
    }

    /// `provider`'s use today.
    pub fn today(&self, provider: &str) -> DayUse {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.days
            .get(&today())
            .and_then(|d| d.get(provider))
            .copied()
            .unwrap_or_default()
    }

    /// Every connection's use today.
    pub fn all_today(&self) -> BTreeMap<String, DayUse> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.days.get(&today()).cloned().unwrap_or_default()
    }

    /// Whether `provider` has used up `limit` (plus today's extra).
    pub fn exhausted(&self, provider: &str, limit: Option<u64>) -> bool {
        let Some(limit) = limit else { return false };
        let d = self.today(provider);
        d.used >= limit.saturating_add(d.extra)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_limits_grants_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("usage.json");
        let l = UsageLedger::open(&path);
        assert!(!l.exhausted("a", Some(100)));
        l.add("a", 60);
        l.add("a", 50);
        assert_eq!(l.today("a").used, 110);
        assert_eq!(l.today("a").calls, 2);
        assert!(l.exhausted("a", Some(100)));
        assert!(!l.exhausted("a", None), "no limit");
        assert!(!l.exhausted("b", Some(100)), "per connection");
        l.grant("a", 25);
        assert!(!l.exhausted("a", Some(100)));
        // Reopened: still there.
        let again = UsageLedger::open(&path);
        assert_eq!(
            again.today("a"),
            DayUse {
                used: 110,
                extra: 25,
                calls: 2
            }
        );
        assert_eq!(again.all_today().len(), 1);
        assert!(until_midnight() <= Duration::from_secs(25 * 3600));
        assert_eq!(estimate_tokens(9), 3);
    }
}
