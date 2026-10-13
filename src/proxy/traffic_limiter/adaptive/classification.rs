//! Persistent IP pressure and bounded authenticated routing hints; no payload inspection.
use super::*;

/// Control-plane state survives individual connection lifetimes.
#[derive(Default)]
pub(super) struct Pressure {
    last_second: u64,
    high_seconds: u64,
    calm_seconds: u64,
    heavy: bool,
    pending_bytes: u64,
    pending_wait: bool,
}

impl Pressure {
    pub(super) fn sample(
        &mut self,
        config: &ShapingConfig,
        now: u64,
        bytes: u64,
        volume: u64,
        waiting: bool,
    ) {
        self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        self.pending_wait |= waiting;
        let elapsed = now.saturating_sub(self.last_second);
        self.last_second = now;
        if config.sustained_secs == 0 {
            *self = Self {
                last_second: now,
                ..Default::default()
            };
            return;
        }
        if elapsed == 0 {
            return;
        }
        // Reload may sample repeatedly in one second; consume the entire interval once.
        let bytes = std::mem::take(&mut self.pending_bytes);
        let waiting = std::mem::take(&mut self.pending_wait);
        // A missed control interval is not evidence of continuous load or recovery.
        if elapsed != 1 {
            self.high_seconds = 0;
            self.calm_seconds = 0;
            return;
        }
        let rate = bytes.saturating_mul(8);
        self.high_seconds = if rate >= config.sustained_rate_bps {
            self.high_seconds.saturating_add(1).min(60)
        } else {
            0
        };
        if !self.heavy
            && self.high_seconds >= config.sustained_secs
            && volume >= config.sustained_min_bytes
        {
            self.heavy = true;
            self.calm_seconds = 0;
        }
        if self.heavy {
            // A throttled sender still waiting for budget must not receive automatic amnesty.
            self.calm_seconds = if !waiting && rate < config.sustained_rate_bps / 2 {
                self.calm_seconds.saturating_add(1)
            } else {
                0
            };
            if self.calm_seconds >= config.sustained_recovery_secs {
                self.heavy = false;
                self.high_seconds = 0;
            }
        }
    }
    pub(super) fn publish(&self, direction: &Direction) {
        direction.sustained.store(self.heavy, Ordering::Release);
        direction
            .sustained_seconds
            .store(self.high_seconds, Ordering::Relaxed);
        direction
            .recovery_seconds
            .store(self.calm_seconds, Ordering::Relaxed);
    }
}

impl Flow {
    pub(super) fn is_bulk(&self, i: usize, config: &ShapingConfig) -> bool {
        if config.sustained_secs > 0 && self.ip.directions[i].sustained.load(Ordering::Acquire) {
            return true;
        }
        // The client selects its DC. This hint never exempts a sustained IP or any budget scope.
        if config.main_dc_priority
            && config.sustained_secs > 0
            && self.main_dc.load(Ordering::Relaxed)
        {
            return false;
        }
        self.directions[i].bulk.load(Ordering::Relaxed)
    }
}

impl TrafficLease {
    /// Stores only an authenticated routing hint, never a claim about encrypted message content.
    pub(crate) fn set_shaping_dc(&self, dc: i16) {
        let _guard = self.refresh.lock();
        let main = (1..=5).contains(&dc);
        self.main_dc.store(main, Ordering::Release);
        if let Some(flow) = self.adaptive.load().as_ref() {
            flow.main_dc.store(main, Ordering::Release);
        }
    }
}
