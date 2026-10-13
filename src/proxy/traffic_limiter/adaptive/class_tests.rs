use super::*;

fn policy() -> ShapingConfig {
    ShapingConfig { enabled: true, up_bps: 800_000, down_bps: 800_000,
        bulk_up_bps: 160_000, bulk_down_bps: 160_000, ..Default::default() }
}
fn consume(flow: &Arc<Flow>, epoch: u64, size: u64) -> u64 {
    let mut reservation = flow.reserve_at_epoch(RateDirection::Down, size, epoch);
    let bytes = reservation.granted;
    reservation.settle(bytes);
    bytes
}

#[test]
fn idle_priority_is_borrowed_and_reclaimed_next_epoch() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let bulk = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    bulk.directions[1].bulk.store(true, Ordering::Relaxed);
    let epoch = 10_000;
    assert_eq!(consume(&bulk, epoch, 2000), 2000);
    let small = owner.acquire("192.0.2.2".parse().unwrap()).unwrap();
    assert_eq!(consume(&small, epoch, 100), 0);
    assert_eq!(consume(&bulk, epoch + 1, 2000), 400);
    assert_eq!(consume(&small, epoch + 1, 1600), 1600);
    assert_eq!(consume(&bulk, epoch + 1, 1), 0);
    assert_eq!(consume(&bulk, epoch + 12, 2000), 2000);
}

#[test]
fn short_flow_on_bulk_ip_reclaims_priority_without_control_tick() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let bulk = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    bulk.directions[1].bulk.store(true, Ordering::Relaxed);
    let d = &bulk.ip.directions[1];
    d.light_cap.store(0, Ordering::Relaxed); d.total_cap.store(400, Ordering::Relaxed);
    bulk.ip.allocation_version.store(owner.admission_version.load(Ordering::Relaxed), Ordering::Relaxed);
    let small = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    assert_eq!(consume(&small, 10_000, 1600), 1600);
    assert_eq!(consume(&bulk, 10_000, 2000), 400);
}

#[test]
fn reconnects_share_priority_and_bulk_scopes() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let epoch = 10_000; let mut bytes = 0;
    for _ in 0..100 {
        let flow = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
        bytes += consume(&flow, epoch, 100);
    }
    assert_eq!(bytes, 2000);
    let state = owner.registry.lock().ips[&"192.0.2.1".parse::<IpAddr>().unwrap()].state.clone();
    assert_eq!(state.directions[1].light_used.state.load(Ordering::Relaxed) & PACKED_USAGE_MASK, 1600);
}

#[test]
fn configured_caps_are_validated_and_directional() {
    let mut config = policy(); config.bulk_down_bps = config.down_bps + 1;
    assert!(config.validate().is_err());
    config.bulk_down_bps = 1; assert!(config.validate().is_err());
    config.bulk_down_bps = 0; assert!(config.validate().is_ok());
    assert_eq!(config.contended_bulk_bps(false), 600_000);
    assert_eq!(config.contended_bulk_bps(true), 160_000);
}

#[test]
fn calculated_class_shares_survive_repeated_first_poller_order() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let a = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    let b = owner.acquire("192.0.2.2".parse().unwrap()).unwrap();
    let version = owner.admission_version.load(Ordering::Relaxed);
    for flow in [&a, &b] {
        flow.ip.allocation_version.store(version, Ordering::Relaxed);
        let d = &flow.ip.directions[1];
        d.light_cap.store(800, Ordering::Relaxed); d.bulk_cap.store(200, Ordering::Relaxed); d.total_cap.store(1000, Ordering::Relaxed);
        d.requested_at.store(10_000, Ordering::Relaxed);
    }
    for epoch in 10_000..10_050 {
        let _idle = owner.acquire("192.0.2.3".parse().unwrap()).unwrap();
        assert_eq!(consume(&a, epoch, 2000), 800);
        assert_eq!(consume(&b, epoch, 2000), 800);
        assert_eq!(consume(&a, epoch, 2000), 200);
        assert_eq!(consume(&b, epoch, 2000), 200);
    }
}

#[test]
fn all_class_scopes_refund_partial_and_cancelled_reservations() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let flow = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    let epoch = 10_000;
    { let mut partial = flow.reserve_at_epoch(RateDirection::Down, 1600, epoch); partial.settle(100); }
    drop(flow.reserve_at_epoch(RateDirection::Down, 1500, epoch));
    assert_eq!(consume(&flow, epoch, 1600), 1500);
    assert_eq!(consume(&flow, epoch, 2000), 400);
    assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed), 2000);
}

#[test]
fn directional_caps_and_reload_preserve_charged_usage() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let flow = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    flow.directions[0].bulk.store(true, Ordering::Relaxed); flow.directions[1].bulk.store(true, Ordering::Relaxed);
    let epoch = 10_000; owner.light_requested[1].store(epoch, Ordering::Relaxed); owner.light_pending[1].store(1, Ordering::Relaxed);
    let mut upload = flow.reserve_at_epoch(RateDirection::Up, 2000, epoch); assert_eq!(upload.granted, 2000); upload.settle(2000);
    assert_eq!(consume(&flow, epoch, 2000), 400);
    let mut reduced = policy(); reduced.bulk_down_bps = 80_000; owner.apply(2, reduced);
    assert_eq!(consume(&flow, epoch, 1), 0);
    assert_eq!(consume(&flow, epoch + 1, 2000), 200);
}

#[test]
fn one_byte_channel_cannot_starve_waiting_priority() {
    let owner = Adaptive::new(); let mut config = policy();
    config.down_bps = 400; config.bulk_down_bps = 0; owner.apply(1, config);
    let bulk = owner.acquire("192.0.2.1".parse().unwrap()).unwrap(); bulk.directions[1].bulk.store(true, Ordering::Relaxed);
    let small = owner.acquire("192.0.2.2".parse().unwrap()).unwrap();
    assert_eq!(consume(&bulk, 10_000, 1), 1);
    assert_eq!(consume(&small, 10_000, 1), 0);
    for epoch in 10_001..10_050 { assert_eq!(consume(&bulk, epoch, 1), 0); assert_eq!(consume(&small, epoch, 100), 1); }
}

#[test]
fn served_small_demand_releases_unused_priority_capacity() {
    let owner = Adaptive::new(); owner.apply(1, policy());
    let small = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    let bulk = owner.acquire("192.0.2.2".parse().unwrap()).unwrap(); bulk.directions[1].bulk.store(true, Ordering::Relaxed);
    let version = owner.admission_version.load(Ordering::Relaxed);
    for (flow, light, remainder) in [(&small, 64, 0), (&bulk, 0, 1936)] {
        flow.ip.allocation_version.store(version, Ordering::Relaxed);
        let d = &flow.ip.directions[1]; d.requested_at.store(10_000, Ordering::Relaxed);
        d.light_cap.store(light, Ordering::Relaxed); d.bulk_cap.store(remainder, Ordering::Relaxed); d.total_cap.store(light + remainder, Ordering::Relaxed);
    }
    for epoch in 10_000..10_050 {
        assert_eq!(consume(&small, epoch, 64), 64);
        assert_eq!(consume(&bulk, epoch, 2000), 1936);
        assert_eq!(owner.light_pending[1].load(Ordering::Relaxed), 0);
    }
}
