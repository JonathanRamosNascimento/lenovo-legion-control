//! Types for `legionctl doctor`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Result of one diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    /// Works.
    Pass,
    /// Works partially / needs attention.
    Warn,
    /// Expected to work but failed.
    Fail,
    /// Not available on this hardware.
    Unsupported,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Unsupported => "UNSUPPORTED",
        })
    }
}

/// One line of the doctor report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Check name.
    pub name: String,
    /// Result.
    pub status: CheckStatus,
    /// Short explanation.
    pub detail: String,
}

impl CheckResult {
    /// Constructor.
    pub fn new(name: &str, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }
}

/// Full doctor report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Checks in display order.
    pub checks: Vec<CheckResult>,
}

impl DoctorReport {
    /// Worst status found (FAIL > WARN > PASS; UNSUPPORTED is neutral).
    pub fn overall(&self) -> CheckStatus {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) {
            CheckStatus::Fail
        } else if self.checks.iter().any(|c| c.status == CheckStatus::Warn) {
            CheckStatus::Warn
        } else {
            CheckStatus::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_status() {
        let mut r = DoctorReport {
            checks: vec![
                CheckResult::new("a", CheckStatus::Pass, ""),
                CheckResult::new("b", CheckStatus::Unsupported, ""),
            ],
        };
        assert_eq!(r.overall(), CheckStatus::Pass);
        r.checks.push(CheckResult::new("c", CheckStatus::Warn, ""));
        assert_eq!(r.overall(), CheckStatus::Warn);
        r.checks.push(CheckResult::new("d", CheckStatus::Fail, ""));
        assert_eq!(r.overall(), CheckStatus::Fail);
        assert_eq!(
            serde_json::to_string(&CheckStatus::Unsupported).unwrap(),
            "\"UNSUPPORTED\""
        );
    }
}
