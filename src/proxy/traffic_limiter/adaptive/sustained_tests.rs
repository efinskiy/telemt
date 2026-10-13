use super::*;

fn policy() -> ShapingConfig {
    ShapingConfig {
        enabled: true,
        up_bps: 800_000,
        down_bps: 800_000,
        bulk_up_bps: 160_000,
        bulk_down_bps: 160_000,
        bulk_idle_up_bps: 400_000,
        bulk_idle_down_bps: 600_000,
        sustained_rate_bps: 8_000,
        sustained_secs: 3,
        sustained_min_bytes: 3_000,
        sustained_recovery_secs: 3,
        ..Default::default()
    }
}

#[test]
fn idle_bulk_ceiling_is_not_the_channel_and_survives_reload() {
    let owner = Adaptive::new();
    assert!(owner.apply(1, policy()));
    let flow = owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    for direction in &flow.directions {
        direction.bulk.store(true, Ordering::Relaxed);
    }
    let mut download = flow.reserve_at_epoch(RateDirection::Down, 2000, 10_000);
    assert_eq!(download.granted, 1500);
    download.settle(1500);
    let mut upload = flow.reserve_at_epoch(RateDirection::Up, 2000, 10_000);
    assert_eq!(upload.granted, 1000);
    upload.settle(1000);
    let mut config = policy();
    config.bulk_idle_down_bps = 400_000;
    assert!(owner.apply(2, config));
    assert_eq!(
        flow.reserve_at_epoch(RateDirection::Down, 2000, 10_000)
            .granted,
        0
    );
    assert_eq!(
        flow.reserve_at_epoch(RateDirection::Down, 2000, 10_001)
            .granted,
        1000
    );
}

#[test]
fn idle_ceiling_and_priority_hint_validation_reject_bypasses() {
    let mut config = policy();
    config.bulk_idle_down_bps = 80_000;
    assert!(config.validate().is_err());
    config = policy();
    config.main_dc_priority = true;
    config.sustained_secs = 0;
    assert!(config.validate().is_err());
}

#[test]
fn sustained_pressure_requires_rate_duration_and_volume_and_recovers() {
    let config = policy();
    let mut pressure = classification::Pressure::default();
    let direction = Direction::default();
    pressure.sample(&config, 1, 100_000, 100_000, false);
    pressure.sample(&config, 2, 0, 100_000, false);
    pressure.publish(&direction);
    assert!(!direction.sustained.load(Ordering::Relaxed));
    for now in 3..=5 {
        pressure.sample(&config, now, 1000, 100_000, false);
    }
    pressure.publish(&direction);
    assert!(direction.sustained.load(Ordering::Relaxed));
    for now in 6..=10 {
        pressure.sample(&config, now, 0, 100_000, true);
    }
    pressure.publish(&direction);
    assert!(direction.sustained.load(Ordering::Relaxed));
    for now in 11..=13 {
        pressure.sample(&config, now, 0, 100_000, false);
    }
    pressure.publish(&direction);
    assert!(!direction.sustained.load(Ordering::Relaxed));
    let mut pressure = classification::Pressure::default();
    for now in 1..=3 {
        pressure.sample(&config, now, 1000, 2999, false);
    }
    pressure.publish(&direction);
    assert!(!direction.sustained.load(Ordering::Relaxed));
    pressure.sample(&config, 10, 100_000, 100_000, false);
    pressure.publish(&direction);
    assert!(!direction.sustained.load(Ordering::Relaxed));
}

#[test]
fn heavy_ip_survives_reconnect_and_dc_hint_without_multiplying_budget() {
    let limiter = TrafficLimiter::new();
    let mut config = policy();
    config.main_dc_priority = true;
    assert!(limiter.apply_shaping_from_source(1, config.clone()));
    let lease = limiter
        .acquire_lease("first", "192.0.2.1".parse().unwrap())
        .unwrap();
    lease.set_shaping_dc(2);
    let flow = lease.adaptive_flow().unwrap();
    flow.directions[1].bulk.store(true, Ordering::Relaxed);
    assert!(!flow.is_bulk(1, &config));
    let mut registry = limiter.adaptive.registry.lock();
    for now in 1..=4 {
        flow.ip.directions[1]
            .committed
            .fetch_add(1000, Ordering::Relaxed);
        control::calculate(&mut registry, now);
    }
    drop(registry);
    assert!(flow.is_bulk(1, &config));
    assert!(!flow.is_bulk(0, &config));
    drop(flow);
    drop(lease);
    let lease = limiter
        .acquire_lease("second", "192.0.2.1".parse().unwrap())
        .unwrap();
    lease.set_shaping_dc(2);
    let flow = lease.adaptive_flow().unwrap();
    assert!(flow.is_bulk(1, &config));
    assert_eq!(
        flow.reserve_at_epoch(RateDirection::Down, 2000, 10_000)
            .granted,
        1500
    );
    assert!(limiter.apply_shaping_from_source(2, config.clone()));
    assert!(flow.is_bulk(1, &config));
    lease.set_shaping_dc(-2);
    assert!(!flow.main_dc.load(Ordering::Relaxed));
}

#[test]
fn route_hint_survives_shaping_enable_and_never_bypasses_ip_budget() {
    let limiter = TrafficLimiter::new();
    let a = limiter
        .acquire_lease("a", "192.0.2.1".parse().unwrap())
        .unwrap();
    a.set_shaping_dc(2);
    let mut config = policy();
    config.main_dc_priority = true;
    assert!(limiter.apply_shaping_from_source(1, config.clone()));
    let b = limiter
        .acquire_lease("b", "192.0.2.1".parse().unwrap())
        .unwrap();
    b.set_shaping_dc(2);
    let a = a.adaptive_flow().unwrap();
    let b = b.adaptive_flow().unwrap();
    assert!(a.main_dc.load(Ordering::Relaxed));
    a.directions[1].bulk.store(true, Ordering::Relaxed);
    b.directions[1].bulk.store(true, Ordering::Relaxed);
    let mut first = a.reserve_at_epoch(RateDirection::Down, 2000, 10_000);
    let n = first.granted;
    first.settle(n);
    let mut second = b.reserve_at_epoch(RateDirection::Down, 2000, 10_000);
    let m = second.granted;
    second.settle(m);
    assert!(n + m <= 2000);
    assert!(Arc::ptr_eq(&a.ip, &b.ip));
}

#[test]
fn same_second_reload_preserves_pressure_samples_and_waits() {
    let config = policy();
    let mut pressure = classification::Pressure::default();
    let direction = Direction::default();
    pressure.sample(&config, 1, 1000, 1000, false);
    for now in 2..=4 {
        pressure.sample(&config, now - 1, 600, 100_000, false);
        pressure.sample(&config, now, 400, 100_000, false);
    }
    pressure.publish(&direction);
    assert!(direction.sustained.load(Ordering::Relaxed));
    for now in 5..=9 {
        pressure.sample(&config, now - 1, 0, 100_000, true);
        pressure.sample(&config, now, 0, 100_000, false);
    }
    pressure.publish(&direction);
    assert!(direction.sustained.load(Ordering::Relaxed));
}

#[test]
fn a_successful_grant_does_not_erase_ip_pressure_before_control_tick() {
    let owner = Adaptive::new();
    let mut config = policy();
    config.sustained_rate_bps = 80_000;
    config.sustained_min_bytes = 30_000;
    assert!(owner.apply(1, config.clone()));
    let address = "192.0.2.4".parse().unwrap();
    let mut flow = owner.acquire(address).unwrap();
    {
        let mut registry = owner.registry.lock();
        for now in 1..=3 {
            flow.ip.directions[1]
                .committed
                .fetch_add(10_000, Ordering::Relaxed);
            control::calculate(&mut registry, now);
        }
    }
    assert!(flow.is_bulk(1, &config));
    for now in 4..=8 {
        let epoch = 10_000 + now * 10;
        let mut denied = false;
        for _ in 0..8 {
            let mut grant = flow.reserve_at_epoch(RateDirection::Down, 2000, epoch);
            let bytes = grant.granted;
            grant.settle(bytes);
            if bytes == 0 {
                denied = true;
                break;
            }
        }
        assert!(denied);
        // Reconnect and a refunded successful grant clear live waits, but not IP demand history.
        drop(flow);
        flow = owner.acquire(address).unwrap();
        assert!(
            flow.reserve_at_epoch(RateDirection::Down, 1, epoch + 1)
                .granted
                > 0
        );
        control::calculate(&mut owner.registry.lock(), now);
    }
    assert!(flow.is_bulk(1, &config));
}
