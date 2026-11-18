//! What the update check remembers between launches: when it asked last and when it may ask
//! again, a version the user skipped, and until when "later" keeps the pill away. It lives in
//! its own file beside the index, not in the settings: it changes by itself every day.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const HOUR: u64 = 60 * 60;
pub const DAY: u64 = 24 * HOUR;
/// "Later" keeps the pill away this long.
pub const LATER: u64 = 3 * DAY;
/// After a failed check (offline, no answer), the next one waits this long.
pub const RETRY: u64 = HOUR;
/// "Check now" asks GitHub at most this often.
pub const MANUAL_GAP: u64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memo {
    /// Unix seconds of the last check that got an answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<u64>,
    /// No automatic check before this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_check: Option<u64>,
    /// GitHub asked to wait until then: no check at all, "Check now" included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limited_until: Option<u64>,
    /// The version the user skipped; a newer one shows again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// "Later": no pill before this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_after: Option<u64>,
}

impl Memo {
    pub fn path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("meshgrove").join("update.json"))
    }

    pub fn load(path: &Path) -> Memo {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            log::warn!("update: could not save {}: {e}", path.display());
        }
    }

    pub fn rate_limited(&self, now: u64) -> bool {
        self.rate_limited_until.is_some_and(|until| now < until)
    }

    /// Time for an automatic check. A next check far in the future means the clock went back:
    /// then it is due too.
    pub fn due(&self, now: u64) -> bool {
        !self.rate_limited(now) && self.next_check.is_none_or(|next| now >= next || next > now + DAY + HOUR)
    }

    /// Seconds until the next automatic check (0: now).
    pub fn wait(&self, now: u64) -> u64 {
        if self.due(now) {
            return 0;
        }
        let next = self.next_check.unwrap_or(now).max(self.rate_limited_until.unwrap_or(0));
        next.saturating_sub(now)
    }

    /// The check got an answer.
    pub fn answered(&mut self, now: u64) {
        self.last_check = Some(now);
        self.next_check = Some(now + DAY);
        self.rate_limited_until = None;
    }

    /// The check got no answer: try again in an hour.
    pub fn failed(&mut self, now: u64) {
        self.next_check = Some(now + RETRY);
    }

    /// GitHub said to wait `seconds` (held between a minute and a day).
    pub fn limited(&mut self, now: u64, seconds: u64) {
        let until = now + seconds.clamp(60, DAY);
        self.rate_limited_until = Some(until);
        self.next_check = Some(until);
    }

    /// Whether the pill stays away for `version`: skipped, or "later" still running.
    pub fn hides(&self, version: &str, now: u64) -> bool {
        self.skipped.as_deref() == Some(version) || self.remind_after.is_some_and(|after| now < after)
    }

    pub fn skip(&mut self, version: &str) {
        self.skipped = Some(version.to_owned());
        self.remind_after = None;
    }

    pub fn later(&mut self, now: u64) {
        self.remind_after = Some(now + LATER);
    }

    /// "Check now" found a newer version: it shows, whatever was skipped or postponed.
    pub fn show_again(&mut self) {
        self.skipped = None;
        self.remind_after = None;
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_at_start_then_daily_and_an_hour_after_a_failure() {
        let mut m = Memo::default();
        let t = 1_800_000_000;
        assert!(m.due(t), "the first launch checks");
        m.answered(t);
        assert!(!m.due(t + DAY - 1));
        assert_eq!(m.wait(t + DAY - 10), 10);
        assert!(m.due(t + DAY));
        m.failed(t + DAY);
        assert!(!m.due(t + DAY + 60) && m.due(t + DAY + RETRY));
    }

    #[test]
    fn a_clock_set_back_does_not_stop_the_checks() {
        let mut m = Memo::default();
        m.answered(1_800_000_000);
        assert!(m.due(1_800_000_000 - 3 * DAY));
    }

    #[test]
    fn waits_as_long_as_github_says_within_bounds() {
        let mut m = Memo::default();
        let t = 1_800_000_000;
        m.limited(t, 5);
        assert!(m.rate_limited(t + 59) && !m.rate_limited(t + 60));
        m.limited(t, 10 * DAY);
        assert_eq!(m.wait(t), DAY);
        assert!(!m.due(t + DAY - 1) && m.due(t + DAY));
        m.answered(t + DAY);
        assert!(!m.rate_limited(t + DAY));
    }

    #[test]
    fn skip_and_later_hide_the_pill() {
        let mut m = Memo::default();
        let t = 1_800_000_000;
        assert!(!m.hides("0.13.0", t));
        m.later(t);
        assert!(m.hides("0.13.0", t + LATER - 1) && !m.hides("0.13.0", t + LATER));
        m.skip("0.13.0");
        assert!(m.hides("0.13.0", t + LATER) && !m.hides("0.14.0", t + LATER));
        m.show_again();
        assert!(!m.hides("0.13.0", t));
    }

    #[test]
    fn survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("update.json");
        assert_eq!(Memo::load(&path), Memo::default());
        let mut m = Memo::default();
        m.answered(42);
        m.skip("0.13.0");
        m.save(&path);
        assert_eq!(Memo::load(&path), m);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Memo::load(&path), Memo::default());
    }
}
