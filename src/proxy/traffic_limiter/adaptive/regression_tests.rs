use super::*;
fn policy() -> ShapingConfig { ShapingConfig { enabled:true, up_bps:100_000_000,down_bps:100_000_000,..Default::default() } }
#[test]
fn idle_resume_receives_bounded_startup_capacity() {
    let owner=Adaptive::new();owner.apply(1,policy());let flow=owner.acquire("192.0.2.1".parse().unwrap()).unwrap();
    flow.directions[1].requested_at.store(0,Ordering::Relaxed);owner.tick();
    assert!(flow.reserve(RateDirection::Down,1024).granted>=1024);
}
#[test]
fn reconnect_cannot_reset_ip_epoch_usage_or_minute_history() {
    let owner=Adaptive::new();owner.apply(1,policy());let address="192.0.2.1".parse().unwrap();let flow=owner.acquire(address).unwrap();
    let mut grant=flow.reserve(RateDirection::Down,4096);grant.settle(4096);let state=flow.ip.clone();drop(flow);
    let replacement=owner.acquire(address).unwrap();assert!(Arc::ptr_eq(&state,&replacement.ip));
    assert_eq!(replacement.ip.directions[1].committed.load(Ordering::Relaxed),4096);
    let d=&replacement.ip.directions[1];let epoch=current_epoch();
    let cap=bytes_per_epoch(policy().down_bps);let mut budget=ReserveCasBudget::new();
    let mut debit=d.total_used.try_reserve_at(epoch,cap,cap,&mut budget).unwrap().unwrap();debit.commit_all();
    assert_eq!(replacement.reserve(RateDirection::Down,1).granted,0);
}
#[test]
fn low_capacity_bulk_still_has_positive_quantum() {
    let owner=Adaptive::new();let mut config=policy();config.down_bps=400;config.priority_percent=99;owner.apply(1,config);
    let flow=owner.acquire("192.0.2.1".parse().unwrap()).unwrap();flow.directions[1].bulk.store(true,Ordering::Relaxed);
    owner.light_requested[1].store(current_epoch(),Ordering::Relaxed);
    assert_eq!(flow.reserve(RateDirection::Down,1).granted,1);
}
#[tokio::test]
async fn partial_write_and_disabled_reload_charge_only_transferred_bytes() {
    use tokio::io::AsyncWriteExt;
    let limiter=TrafficLimiter::new();limiter.apply_shaping_from_source(1,policy());let lease=limiter.acquire_lease("a","192.0.2.1".parse().unwrap()).unwrap();let flow=lease.adaptive_flow().unwrap();
    let (stream,_peer)=tokio::io::duplex(7);let mut shaped=AdaptiveIo::new(stream,Some(lease));
    assert_eq!(shaped.write(&[1;64]).await.unwrap(),7);assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed),7);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(5),shaped.write(&[1;64])).await.is_err());
    assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed),7);
    limiter.apply_shaping_from_source(2,ShapingConfig::default());assert!(!limiter.adaptive.enabled());
}
#[tokio::test(flavor="multi_thread",worker_threads=4)]
#[ignore = "reproducible 10,000-flow load check"]
async fn ten_thousand_flows_preserve_budget_and_small_flow_latency() {
    let limiter=TrafficLimiter::new();let mut config=policy();config.down_bps=1_000_000_000;limiter.apply_shaping_from_source(1,config.clone());
    let mut flows=Vec::new();for n in 0..10_000u32 {let ip=IpAddr::V4(std::net::Ipv4Addr::from(0x0a000001+n));flows.push(limiter.acquire_lease("load",ip).unwrap().adaptive_flow().unwrap());}
    for flow in &flows[..9000] {flow.directions[1].committed.store(config.bulk_enter_bytes,Ordering::Relaxed);flow.ip.directions[1].committed.store(config.bulk_enter_bytes,Ordering::Relaxed);flow.directions[1].bulk.store(true,Ordering::Relaxed);}
    let started=std::time::Instant::now();limiter.tick_shaping();let calculation=started.elapsed();
    let mut tasks=tokio::task::JoinSet::new();
    for flow in flows {tasks.spawn(async move{let small=!flow.directions[1].bulk.load(Ordering::Relaxed);let start=tokio::time::Instant::now();let mut remaining=if small{512}else{32_768};let deadline=start+std::time::Duration::from_secs(2);while remaining>0 {let mut reservation=flow.reserve(RateDirection::Down,remaining);let bytes=reservation.granted;reservation.settle(bytes);remaining-=bytes;if remaining==0||tokio::time::Instant::now()>=deadline{break;}tokio::time::sleep(next_refill_delay()).await;} (small,start.elapsed().as_millis() as u64)});}
    let mut latency=Vec::new();while let Some(result)=tasks.join_next().await{let(small,ms)=result.unwrap();if small{latency.push(ms);}}
    assert_eq!(latency.len(),1000);latency.sort_unstable();let p95=latency[latency.len()*95/100];let snapshot=limiter.shaping_snapshot(0,1);
    println!("flows=10000 small=1000 p95_ms={p95} calculation_ms={} retained_ips={} committed_down={}",calculation.as_millis(),snapshot.retained_ips,snapshot.committed_bytes[1]);
    assert!(p95<=500,"small-flow p95={p95}ms");assert_eq!(snapshot.retained_ips,10_000);
    assert!(snapshot.committed_bytes[1].saturating_sub(9000*config.bulk_enter_bytes)<=config.down_bps/8*3);
}

#[test]
fn idle_connections_do_not_dilute_one_resumed_ip() {
    let owner=Adaptive::new();let mut config=policy();config.down_bps=1_000_000;owner.apply(1,config);
    let mut flows=Vec::new();for n in 0..10_000u32 {flows.push(owner.acquire(IpAddr::V4(std::net::Ipv4Addr::from(0x0a000001+n))).unwrap());}
    for flow in &flows { flow.directions[1].requested_at.store(0,Ordering::Relaxed); }
    owner.tick();assert_eq!(flows[0].reserve(RateDirection::Down,512).granted,512);
}

#[tokio::test]
async fn live_enable_disable_preserves_binding_and_history_without_telemetry() {
    use tokio::io::AsyncWriteExt;
    let limiter=TrafficLimiter::new();let lease=limiter.acquire_lease("existing","192.0.2.1".parse().unwrap()).unwrap();
    assert!(lease.adaptive_flow().is_none());let (stream,_peer)=tokio::io::duplex(64);let mut shaped=AdaptiveIo::new(stream,Some(lease.clone()));
    assert!(limiter.apply_shaping_from_source(2,policy()));shaped.write_all(&[1;2]).await.unwrap();let flow=lease.adaptive_flow().unwrap();
    assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed),2);
    assert!(!limiter.apply_shaping_from_source(1,ShapingConfig::default()));
    assert!(limiter.apply_shaping_from_source(3,ShapingConfig::default()));shaped.write_all(&[1;3]).await.unwrap();
    assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed),2);
    assert!(limiter.apply_shaping_from_source(4,policy()));assert!(Arc::ptr_eq(&flow,&lease.adaptive_flow().unwrap()));
    shaped.write_all(&[1;3]).await.unwrap();assert_eq!(flow.ip.directions[1].committed.load(Ordering::Relaxed),5);
}

#[test]
fn weighted_ip_epoch_credit_is_frozen_and_never_multiplied_by_connections() {
    let owner=Adaptive::new();owner.apply(1,policy());let flow=owner.acquire("192.0.2.1".parse().unwrap()).unwrap();let d=&flow.ip.directions[1];let epoch=12345;
    for weight in [1,16,16,16] {let mut budget=ReserveCasBudget::new();assert!(d.demand.ensure_active_weighted(epoch,&owner.demand[1],&mut budget,weight).unwrap());}
    assert_eq!(d.demand.used.state.load(Ordering::Relaxed)&PACKED_USAGE_MASK,1);
    assert_eq!(owner.demand[1].state.load(Ordering::Relaxed)&PACKED_USAGE_MASK,1);
    let other=Adaptive::new();assert_ne!(owner.snapshot(0,0).counter_instance,other.snapshot(0,0).counter_instance);
}
