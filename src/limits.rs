//! Account rate-limit snapshots (currently reported by Codex).

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    /// Unix ms.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct RateLimits {
    /// When this snapshot was taken (unix ms).
    pub ts: i64,
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
    pub plan: Option<String>,
}
