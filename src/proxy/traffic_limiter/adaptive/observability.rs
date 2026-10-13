//! Account measurements reuse flow atomics; account identity is allocated only at admission.
use super::*;
use std::collections::BTreeMap;

/// Per-account measurements of currently live authenticated flows, not lifetime totals.
#[derive(Serialize, Default)]
pub(crate) struct UserShapingSnapshot {
    /// Authenticated account, independent of shared NAT addresses.
    pub username: String,
    /// Number of live logical flows.
    pub flows: usize,
    /// Successfully transferred bytes in the live flows' rolling minute windows.
    pub window_bytes: [u64; 2],
    /// Successfully transferred bytes over the lifetimes of the live flows.
    pub committed_bytes: [u64; 2],
    /// Last controller sample's directional rates in bits per second.
    pub speed_bps: [u64; 2],
    /// Live flows assigned to the bulk class in each direction.
    pub bulk_flows: [usize; 2],
    /// Live flows currently waiting for an adaptive budget.
    pub waiting_flows: [usize; 2],
    /// Cumulative pacing time belonging to live flows only.
    pub wait_ms: [u64; 2],
}

pub(super) fn users(registry: &Registry) -> Vec<UserShapingSnapshot> {
    let mut users = BTreeMap::<&str, UserShapingSnapshot>::new();
    let epoch = current_epoch();
    // MAX_FLOWS bounds this traversal and the number of account aggregates.
    let flows: Vec<_> = registry.ips.values().flat_map(|entry| entry.flows.iter().filter_map(|(flow, _)| flow.upgrade())).collect();
    for flow in &flows {
        let user = users.entry(&flow.username).or_insert_with(|| UserShapingSnapshot { username: flow.username.clone(), ..Default::default() });
        user.flows += 1;
        for i in 0..2 {
            user.window_bytes[i] = user.window_bytes[i].saturating_add(flow.directions[i].window.load(Ordering::Relaxed));
            user.committed_bytes[i] = user.committed_bytes[i].saturating_add(flow.directions[i].committed.load(Ordering::Relaxed));
            user.speed_bps[i] = user.speed_bps[i].saturating_add(flow.directions[i].speed.load(Ordering::Relaxed));
            user.wait_ms[i] = user.wait_ms[i].saturating_add(flow.wait_ms[i].load(Ordering::Relaxed));
            user.bulk_flows[i] += usize::from(flow.directions[i].bulk.load(Ordering::Relaxed));
            user.waiting_flows[i] += usize::from(registry.config.enabled && flow.waiting_until[i].load(Ordering::Relaxed) > epoch);
        }
    }
    users.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_ip_accounts_wait_independently_and_totals_ignore_pagination() {
        let limiter = TrafficLimiter::new();
        limiter.apply_shaping_from_source(1, ShapingConfig { enabled: true, up_bps: 800_000, down_bps: 800_000, ..Default::default() });
        let a = limiter.acquire_lease("alice", "192.0.2.1".parse().unwrap()).unwrap();
        let b = limiter.acquire_lease("bob", "192.0.2.1".parse().unwrap()).unwrap();
        let flow = a.adaptive_flow().unwrap();
        flow.waiting_until[1].store(current_epoch() + 1000, Ordering::Relaxed);
        flow.wait_ms[1].store(123, Ordering::Relaxed);
        flow.directions[1].committed.store(456, Ordering::Relaxed);
        let other = b.adaptive_flow().unwrap();
        other.directions[1].bulk.store(true, Ordering::Relaxed);
        let page = limiter.shaping_snapshot(1, 1);
        assert_eq!((page.active_users, page.waiting_users, page.active_flows), (2, 1, 2));
        assert_eq!(page.users[0].username, "bob");
        assert_eq!(page.users[0].waiting_flows, [0, 0]);
        assert_eq!(page.users[0].committed_bytes, [0, 0]);
        let all = limiter.shaping_snapshot(0, 100);
        assert_eq!(all.users[0].wait_ms, [0, 123]);
        assert_eq!(all.users[0].committed_bytes, [0, 456]);
        assert_eq!(all.ips[0].waiting_flows, [0, 1]);
        limiter.apply_shaping_from_source(2, ShapingConfig::default());
        assert_eq!(limiter.shaping_snapshot(0, 100).waiting_users, 0);
    }

    #[test]
    fn lazy_enablement_retains_identity_and_closed_flows_leave_live_measurements() {
        let limiter = TrafficLimiter::new();
        let lease = limiter.acquire_lease("alice", "192.0.2.1".parse().unwrap()).unwrap();
        limiter.apply_shaping_from_source(1, ShapingConfig { enabled: true, up_bps: 800_000, down_bps: 800_000, ..Default::default() });
        let flow = lease.adaptive_flow().unwrap();
        assert_eq!(limiter.shaping_snapshot(0, 1).users[0].username, "alice");
        drop(flow); drop(lease);
        assert_eq!(limiter.shaping_snapshot(0, 1).active_users, 0);
    }
}
