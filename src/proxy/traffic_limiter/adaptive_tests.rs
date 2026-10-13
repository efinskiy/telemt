use super::*;

fn configured() -> crate::config::ShapingConfig {
    crate::config::ShapingConfig {
        enabled: true,
        up_bps: 800_000,
        down_bps: 800_000,
        ..Default::default()
    }
}

#[test]
fn adaptive_allocator_is_bounded_and_work_conserving() {
    let demands = [(1.0, 10), (0.25, 100), (0.0625, 100)];
    let rates = adaptive::allocate(90, &demands);
    assert_eq!(rates.iter().sum::<u64>(), 90);
    assert_eq!(rates[0], 10);
    assert!(rates[1] >= rates[2]);
}

#[test]
fn adaptive_same_ip_does_not_multiply_budget_and_refunds_partial_io() {
    let limiter = TrafficLimiter::new();
    limiter.apply_shaping_from_source(1, configured());
    let a = limiter.acquire_lease("alice", "192.0.2.1".parse().unwrap()).unwrap();
    let b = limiter.acquire_lease("bob", "::ffff:192.0.2.1".parse().unwrap()).unwrap();
    let fa = a.adaptive_flow().unwrap();
    let fb = b.adaptive_flow().unwrap();
    assert!(Arc::ptr_eq(&fa.ip, &fb.ip));
    let mut grant = fa.reserve(RateDirection::Down, 100);
    assert_eq!(grant.granted, 100);
    grant.settle(25);
    assert_eq!(fa.ip.directions[1].committed.load(Ordering::Relaxed), 25);
    assert_eq!(fa.directions[1].committed.load(Ordering::Relaxed), 25);
}

#[test]
fn adaptive_table_exhaustion_has_no_unlimited_fallback() {
    let limiter = TrafficLimiter::new();
    let mut config = configured();
    config.max_ips = 1;
    limiter.apply_shaping_from_source(2, config);
    assert!(limiter.acquire_lease("a", "192.0.2.1".parse().unwrap()).is_some());
    assert!(limiter.acquire_lease("b", "192.0.2.2".parse().unwrap()).is_none());
    assert!(!limiter.apply_shaping_from_source(1, Default::default()));
    assert!(limiter.adaptive.enabled());
}

#[test]
fn adaptive_window_expires_without_wall_clock_or_reconnect_reset() {
    let mut window = adaptive::Window::default();
    assert_eq!(window.sample(1, 100), (100, 100));
    assert_eq!(window.sample(60, 100), (100, 0));
    assert_eq!(window.sample(61, 100), (0, 0));
    assert_eq!(window.sample(121, 120), (20, 20));
}

#[tokio::test]
async fn adaptive_io_pending_and_cancellation_do_not_charge_usage() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let limiter = TrafficLimiter::new();
    limiter.apply_shaping_from_source(1, configured());
    let lease = limiter.acquire_lease("a", "192.0.2.1".parse().unwrap()).unwrap();
    let flow = lease.adaptive_flow().unwrap();
    let (client, mut other) = tokio::io::duplex(64);
    let mut shaped = AdaptiveIo::new(client, Some(lease));
    let mut bytes = [0u8; 16];
    assert!(tokio::time::timeout(std::time::Duration::from_millis(5), shaped.read(&mut bytes)).await.is_err());
    assert_eq!(flow.ip.directions[0].committed.load(Ordering::Relaxed), 0);
    other.write_all(&[7u8; 16]).await.unwrap();
    assert_eq!(shaped.read(&mut bytes).await.unwrap(), 16);
    assert_eq!(flow.ip.directions[0].committed.load(Ordering::Relaxed), 16);
}
