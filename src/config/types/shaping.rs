use super::*;

/// Optional process-local IP shaping policy; capacity is the usable authenticated channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ShapingConfig {
    /// Enables adaptive shaping without changing transport or cryptography.
    pub enabled: bool,
    /// Usable client upload capacity, in bits per second.
    pub up_bps: u64,
    /// Usable client download capacity, in bits per second.
    pub down_bps: u64,
    /// Bulk upload ceiling during priority demand; zero derives it from priority_percent.
    pub bulk_up_bps: u64,
    /// Bulk download ceiling during priority demand; idle priority capacity remains borrowable.
    pub bulk_down_bps: u64,
    /// Percentage reserved for small flows while both classes have demand.
    pub priority_percent: u8,
    /// Rolling directional bytes at which a flow becomes bulk.
    pub bulk_enter_bytes: u64,
    /// Rolling directional bytes below which a bulk flow becomes small again.
    pub bulk_exit_bytes: u64,
    /// Bound on retained IP records, including disconnected records in the minute window.
    pub max_ips: usize,
}

impl Default for ShapingConfig {
    fn default() -> Self {
        Self {
            enabled: false, up_bps: 0, down_bps: 0, bulk_up_bps: 0, bulk_down_bps: 0, priority_percent: 25,
            bulk_enter_bytes: 256 * 1024, bulk_exit_bytes: 64 * 1024, max_ips: 32_768,
        }
    }
}

impl ShapingConfig {
    /// Effective bulk ceiling under contention, bounded by the guaranteed priority share.
    pub(crate) fn contended_bulk_bps(&self, upload: bool) -> u64 {
        let (capacity, configured) = if upload { (self.up_bps, self.bulk_up_bps) } else { (self.down_bps, self.bulk_down_bps) };
        let remainder = capacity.saturating_mul(u64::from(100u8.saturating_sub(self.priority_percent))) / 100;
        if configured == 0 { remainder } else { configured.min(remainder) }
    }
    /// Rejects unsupported precision, unbounded state and contradictory thresholds.
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && (!(400..=MAX_RATE_LIMIT_BPS).contains(&self.up_bps)
            || !(400..=MAX_RATE_LIMIT_BPS).contains(&self.down_bps)) {
            return Err("shaping capacities must be within [400, 100000000000] bps when enabled".into());
        }
        for (bulk, capacity) in [(self.bulk_up_bps, self.up_bps), (self.bulk_down_bps, self.down_bps)] {
            if bulk != 0 && (!(400..=MAX_RATE_LIMIT_BPS).contains(&bulk) || bulk > capacity) {
                return Err("shaping bulk caps must be zero or within [400, directional capacity] bps".into());
            }
        }
        if !(1..=99).contains(&self.priority_percent) || !(1..=131_072).contains(&self.max_ips)
            || self.bulk_exit_bytes == 0 || self.bulk_exit_bytes >= self.bulk_enter_bytes
            || self.bulk_enter_bytes > 1024 * 1024 * 1024 {
            return Err("invalid shaping priority, table bound or bulk thresholds".into());
        }
        Ok(())
    }
}
