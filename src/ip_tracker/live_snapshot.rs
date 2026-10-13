//! Bounded control-plane views of existing admissions, independent of traffic telemetry.
use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// One normalized source IP shared by any number of authenticated accounts.
#[derive(Debug, Serialize)]
pub(crate) struct LiveClientIp {
    /// Address established by the trusted client identity path.
    pub ip: IpAddr,
    /// Live authenticated admissions, including logical WEB streams.
    pub connections: usize,
    /// Distinct account/IP pairs represented at this address.
    pub users: usize,
}

/// A sequential shard observation, not a globally atomic census.
#[derive(Debug, Serialize)]
pub(crate) struct LiveClientsSnapshot {
    /// Sorted bounded page of active addresses only.
    pub ips: Vec<LiveClientIp>,
    /// Total addresses in the observed shards before pagination.
    pub total_ips: usize,
    /// Total positive admissions in the observation.
    pub connections: usize,
    /// Accounts with positive admissions; empty rejected-admission entries are excluded.
    pub active_users: usize,
    /// Deferred release tuples remaining after the bounded cleanup pass.
    pub cleanup_pending: u64,
    /// Whether scanning or response limits omitted part of the observation.
    pub truncated: bool,
}

impl UserIpTracker {
    /// Reads existing admission state without adding work to packet or handshake accounting.
    pub(crate) async fn live_clients(&self, limit: usize) -> LiveClientsSnapshot {
        self.drain_cleanup_queue().await;
        let mut ips = BTreeMap::<IpAddr, LiveClientIp>::new();
        let mut scanned = 0usize;
        let mut scanned_users = 0usize;
        let mut connections = 0usize;
        let mut active_users = 0usize;
        let mut truncated = false;
        // Churn between sequential shard reads must not grow a composite snapshot unboundedly.
        'shards: for shard in self.shards.iter() {
            let shard = shard.read().await;
            for entries in shard.active_ips.values() {
                if scanned_users >= MAX_ACTIVE_IP_ENTRIES as usize {
                    truncated = true;
                    break 'shards;
                }
                scanned_users += 1;
                let mut active = false;
                for (&ip, &count) in entries {
                    if scanned >= MAX_ACTIVE_IP_ENTRIES as usize {
                        truncated = true;
                        break 'shards;
                    }
                    scanned += 1;
                    if count == 0 {
                        continue;
                    }
                    if !active {
                        active_users = active_users.saturating_add(1);
                        active = true;
                    }
                    let row = ips.entry(ip).or_insert(LiveClientIp {
                        ip,
                        connections: 0,
                        users: 0,
                    });
                    row.connections = row.connections.saturating_add(count);
                    row.users = row.users.saturating_add(1);
                    connections = connections.saturating_add(count);
                }
            }
        }
        let total_ips = ips.len();
        let limit = limit.min(10_000);
        LiveClientsSnapshot {
            ips: ips.into_values().take(limit).collect(),
            total_ips,
            connections,
            active_users,
            cleanup_pending: self.cleanup_queue_len.load(Ordering::Relaxed),
            truncated: truncated || total_ips > limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_shared_nat_multiple_ips_and_positive_admissions_before_limit() {
        let tracker = UserIpTracker::new();
        // Exercise the production data shape without relying on optional telemetry or a shaper.
        let mut shard = tracker.shards[0].write().await;
        shard.active_ips.insert(
            "alice".into(),
            [
                ("192.0.2.1".parse().unwrap(), 3),
                ("2001:db8::1".parse().unwrap(), 2),
            ]
            .into(),
        );
        shard
            .active_ips
            .insert("bob".into(), [("192.0.2.1".parse().unwrap(), 1)].into());
        shard.active_ips.insert("rejected".into(), HashMap::new());
        shard
            .active_ips
            .insert("zero".into(), [("192.0.2.3".parse().unwrap(), 0)].into());
        drop(shard);
        let snapshot = tracker.live_clients(1).await;
        assert_eq!(
            (
                snapshot.total_ips,
                snapshot.connections,
                snapshot.active_users
            ),
            (2, 6, 2)
        );
        assert_eq!((snapshot.ips[0].connections, snapshot.ips[0].users), (4, 2));
        assert!(snapshot.truncated);
        assert_eq!(snapshot.cleanup_pending, 0);
        assert!(!tracker.live_clients(10_000).await.truncated);
    }

    #[tokio::test]
    async fn disconnect_cleanup_is_reflected_in_the_next_snapshot() {
        let tracker = UserIpTracker::new();
        let ip = "1.1.1.1".parse().unwrap();
        tracker.check_and_add("alice", ip).await.unwrap();
        tracker.check_and_add("alice", ip).await.unwrap();
        assert_eq!(tracker.live_clients(10_000).await.connections, 2);
        tracker.enqueue_cleanup("alice".into(), ip);
        assert_eq!(tracker.live_clients(10_000).await.connections, 1);
        tracker.enqueue_cleanup("alice".into(), ip);
        let snapshot = tracker.live_clients(10_000).await;
        assert_eq!(
            (
                snapshot.connections,
                snapshot.active_users,
                snapshot.total_ips
            ),
            (0, 0, 0)
        );
    }

    #[tokio::test]
    async fn recent_history_is_not_a_live_connection() {
        let tracker = UserIpTracker::new();
        tracker.shards[0].write().await.recent_ips.insert(
            "alice".into(),
            [("192.0.2.1".parse().unwrap(), Instant::now())].into(),
        );
        let snapshot = tracker.live_clients(10_000).await;
        assert_eq!(
            (
                snapshot.total_ips,
                snapshot.connections,
                snapshot.active_users
            ),
            (0, 0, 0)
        );
        assert!(snapshot.ips.is_empty());
    }
}
