//! Process-owned IP and flow state. Registry locks are used only for admission and control ticks.
use super::*;
use crate::config::ShapingConfig;
use std::sync::{Weak, atomic::AtomicBool};
use serde::Serialize;

mod control;
mod io;
pub(crate) use io::AdaptiveIo;
pub(super) use control::Window;
#[cfg(test)]
pub(super) use control::allocate;
const WINDOW: usize = 60;
// Bounds flow window memory independently of the retained IP admission limit.
const MAX_FLOWS: usize = 65_536;

/// Process-owned directional counters and IP reservation scopes.
pub(super) struct Direction {
    pub(super) committed: AtomicU64,
    pub(super) requested_at: AtomicU64,
    window: AtomicU64,
    speed: AtomicU64,
    light_cap: AtomicU64,
    bulk_cap: AtomicU64,
    total_cap: AtomicU64,
    light_chunk: AtomicU64,
    weight: AtomicU64,
    demand: CidrUserDirectionState,
    light_demand: CidrUserDirectionState,
    total_used: Arc<DirectionBucket>,
    light_used: Arc<DirectionBucket>,
    bulk_used: Arc<DirectionBucket>,
}

impl Default for Direction {
    fn default() -> Self {
        Self { committed: AtomicU64::new(0), requested_at: AtomicU64::new(0), window: AtomicU64::new(0), speed: AtomicU64::new(0),
            light_cap: AtomicU64::new(0), bulk_cap: AtomicU64::new(0), total_cap: AtomicU64::new(0), light_chunk: AtomicU64::new(0), weight: AtomicU64::new(16),
            demand: CidrUserDirectionState { used: Arc::new(DirectionBucket::default()) },
            light_demand: CidrUserDirectionState { used: Arc::new(DirectionBucket::default()) },
            total_used: Arc::new(DirectionBucket::default()), light_used: Arc::new(DirectionBucket::default()), bulk_used: Arc::new(DirectionBucket::default()) }
    }
}

struct IpHistory {
    state: Arc<IpState>,
    history: [Window; 2],
    flows: Vec<(Weak<Flow>, [Window; 2])>,
}

/// Shared by every authenticated flow from one normalized client address.
pub(super) struct IpState {
    pub(super) directions: [Direction; 2],
    last_io: AtomicU64,
    flows: AtomicU64,
    allocation_version: AtomicU64,
}

/// Flow hysteresis never changes payload order or IP ownership.
pub(super) struct FlowDirection {
    pub(super) committed: AtomicU64,
    window: AtomicU64,
    bulk: AtomicBool,
    requested_at: AtomicU64,
    pending_bytes: AtomicU64,
    priority_pending: AtomicBool,
}

/// One logical relay owns both directions and cancellation-safe wait accounting.
pub(super) struct Flow {
    owner: Arc<Adaptive>,
    pub(super) ip: Arc<IpState>,
    pub(super) directions: [FlowDirection; 2],
    waiting_until: [AtomicU64; 2],
    wait_ms: [AtomicU64; 2],
}

#[derive(Default)]
struct Registry {
    generation: u64,
    config: ShapingConfig,
    tick: u64,
    last_cleanup: u64,
    retired: [u64; 2],
    ips: HashMap<IpAddr, IpHistory>,
    flows: usize,
}

/// Separates admission/control state from lock-free I/O reservations.
pub(super) struct Adaptive {
    counter_instance: String,
    config: ArcSwap<ShapingConfig>,
    enabled: AtomicBool,
    registry: ParkingMutex<Registry>,
    global: [Arc<DirectionBucket>; 2],
    bulk: [Arc<DirectionBucket>; 2],
    demand: [Arc<DirectionBucket>; 2],
    light_requested: [AtomicU64; 2],
    light_pending: [AtomicU64; 2],
    light_demand: [Arc<DirectionBucket>; 2],
    light: [Arc<DirectionBucket>; 2],
    active_ips: AtomicU64,
    admission_version: AtomicU64,
    exhausted: AtomicU64,
    throttles: AtomicU64,
    waits_ms: AtomicU64,
}

#[derive(Serialize)]
pub(crate) struct ShapingSnapshot {
    /// Process-owned counter identity, independent of configuration generations.
    pub counter_instance: String,
    /// Whether the process-local policy is enabled.
    pub enabled: bool,
    /// Current usable directional capacities and thresholds.
    pub policy: ShapingConfig,
    /// Number of retained IPs, including minute-window tombstones.
    pub retained_ips: usize,
    /// Cumulative successful bytes, retained across config reload and expired IP cleanup.
    pub committed_bytes: [u64; 2],
    /// Monotonic age of the most recent controller calculation.
    pub calculation_age_ms: u64,
    /// Rejected state admissions; never an unlimited fallback.
    pub admission_rejected_total: u64,
    /// Budget refusals across adaptive scopes.
    pub throttle_total: u64,
    /// Accumulated pacing waits.
    pub wait_ms_total: u64,
    /// Bounded, IP-sorted page of process state.
    pub ips: Vec<IpSnapshot>,
}

#[derive(Serialize)]
pub(crate) struct IpSnapshot {
    /// Normalized client address; no ports or user secrets.
    pub ip: IpAddr,
    /// Directional bytes successfully transferred in the minute window.
    pub window_bytes: [u64; 2],
    /// Last one-second measured directional rates in bits/s.
    pub speed_bps: [u64; 2],
    /// Directional small/bulk grants per 20 ms epoch.
    pub epoch_bytes: [[u64; 2]; 2],
    /// Live logical flow count and directional bulk flow count.
    pub flows: usize,
    /// Live bulk classifications for upload and download.
    pub bulk_flows: [usize; 2],
}

/// Refunds every unused debit when I/O is partial, pending, failed or cancelled.
pub(super) struct AdaptiveReservation {
    pub(super) granted: u64,
    debits: [Option<DirectionDebit>; 5],
    flow: Arc<Flow>,
    index: usize,
    requested: u64,
    settled: bool,
}

fn index(direction: RateDirection) -> usize { match direction { RateDirection::Up => 0, RateDirection::Down => 1 } }
fn second() -> u64 { helpers::limiter_epoch_start().elapsed().as_secs() + 1 }

impl Adaptive {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            counter_instance: format!("{}:{:?}", std::process::id(), std::time::Instant::now()),
            config: ArcSwap::from_pointee(ShapingConfig::default()), enabled: AtomicBool::new(false), registry: ParkingMutex::new(Registry::default()),
            global: std::array::from_fn(|_| Arc::new(DirectionBucket::default())),
            bulk: std::array::from_fn(|_| Arc::new(DirectionBucket::default())),
            demand: std::array::from_fn(|_| Arc::new(DirectionBucket::default())),
            light_requested: std::array::from_fn(|_| AtomicU64::new(0)),
            light_pending: std::array::from_fn(|_| AtomicU64::new(0)),
            light_demand: std::array::from_fn(|_| Arc::new(DirectionBucket::default())),
            light: std::array::from_fn(|_| Arc::new(DirectionBucket::default())),
            active_ips: AtomicU64::new(0), admission_version: AtomicU64::new(1), exhausted: AtomicU64::new(0), throttles: AtomicU64::new(0), waits_ms: AtomicU64::new(0),
        })
    }
    pub(super) fn enabled(&self) -> bool { self.enabled.load(Ordering::Acquire) }
    pub(super) fn apply(&self, generation: u64, config: ShapingConfig) -> bool {
        let mut registry = self.registry.lock();
        if generation < registry.generation || config.validate().is_err() { return false; }
        registry.generation = generation;
        if config != registry.config {
            registry.config = config.clone();
            let enabled = config.enabled;
            self.config.store(Arc::new(config));
            self.enabled.store(enabled, Ordering::Release);
            registry.tick = 0;
        }
        true
    }
    pub(super) fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<Arc<Flow>> {
        let ip = match ip { IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ip)), ip => ip };
        let mut registry = self.registry.lock();
        if registry.last_cleanup != second() { control::cleanup(&mut registry, second()); registry.last_cleanup = second(); }
        if registry.flows >= MAX_FLOWS || (!registry.ips.contains_key(&ip) && registry.ips.len() >= registry.config.max_ips) {
            self.exhausted.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let entry = registry.ips.entry(ip).or_insert_with(|| IpHistory {
            state: Arc::new(IpState { directions: std::array::from_fn(|_| Direction::default()), last_io: AtomicU64::new(second()), flows: AtomicU64::new(0), allocation_version: AtomicU64::new(0) }),
            history: std::array::from_fn(|_| Window::default()), flows: Vec::new(),
        });
        if entry.state.flows.fetch_add(1, Ordering::Relaxed) == 0 {
            self.active_ips.fetch_add(1, Ordering::Relaxed);
            self.admission_version.fetch_add(1, Ordering::Release);
        }
        let flow = Arc::new(Flow {
            owner: Arc::clone(self), ip: Arc::clone(&entry.state), waiting_until: std::array::from_fn(|_| AtomicU64::new(0)), wait_ms: std::array::from_fn(|_| AtomicU64::new(0)),
            directions: std::array::from_fn(|_| FlowDirection {
                committed: AtomicU64::new(0), window: AtomicU64::new(0), bulk: AtomicBool::new(false),
                requested_at: AtomicU64::new(current_epoch()), pending_bytes: AtomicU64::new(0), priority_pending: AtomicBool::new(false),
            }),
        });
        entry.flows.push((Arc::downgrade(&flow), std::array::from_fn(|_| Window::default())));
        registry.flows += 1;
        let count = self.active_ips.load(Ordering::Relaxed).max(1);
        let config = &registry.config;
        for i in 0..2 {
            let cap = bytes_per_epoch(if i == 0 { config.up_bps } else { config.down_bps });
            let d = &flow.ip.directions[i];
            if d.total_cap.load(Ordering::Relaxed) == 0 { d.total_cap.store((cap / count).max(1), Ordering::Release); d.light_cap.store((cap / count).max(1), Ordering::Release); }
        }
        Some(flow)
    }
    pub(super) fn tick(&self) {
        let now = second();
        let mut registry = self.registry.lock();
        if registry.tick == now { return; }
        control::cleanup(&mut registry, now);
        let version = self.admission_version.load(Ordering::Acquire);
        control::calculate(&mut registry, now);
        for entry in registry.ips.values() { entry.state.allocation_version.store(version, Ordering::Release); }
        registry.tick = now;
    }
    pub(super) fn snapshot(&self, offset: usize, limit: usize) -> ShapingSnapshot {
        let registry = self.registry.lock();
        let mut ips: Vec<_> = registry.ips.iter().collect();
        ips.sort_unstable_by_key(|(ip, _)| **ip);
        ShapingSnapshot {
            counter_instance: self.counter_instance.clone(),
            enabled: registry.config.enabled, policy: registry.config.clone(), retained_ips: ips.len(),
            committed_bytes: std::array::from_fn(|i| ips.iter().fold(registry.retired[i], |sum, (_, entry)| sum.saturating_add(entry.state.directions[i].committed.load(Ordering::Relaxed)))),
            calculation_age_ms: second().saturating_sub(registry.tick).saturating_mul(1000),
            admission_rejected_total: self.exhausted.load(Ordering::Relaxed), throttle_total: self.throttles.load(Ordering::Relaxed),
            wait_ms_total: self.waits_ms.load(Ordering::Relaxed),
            ips: ips.into_iter().skip(offset).take(limit.min(1000)).map(|(ip, entry)| IpSnapshot {
                ip: *ip, window_bytes: std::array::from_fn(|i| entry.state.directions[i].window.load(Ordering::Relaxed)),
                speed_bps: std::array::from_fn(|i| entry.state.directions[i].speed.load(Ordering::Relaxed)),
                epoch_bytes: std::array::from_fn(|i| [entry.state.directions[i].light_cap.load(Ordering::Relaxed), entry.state.directions[i].bulk_cap.load(Ordering::Relaxed)]),
                flows: entry.flows.iter().filter(|(flow, _)| flow.strong_count() > 0).count(),
                bulk_flows: std::array::from_fn(|i| entry.flows.iter().filter_map(|(flow, _)| flow.upgrade()).filter(|flow| flow.directions[i].bulk.load(Ordering::Relaxed)).count()),
            }).collect(),
        }
    }
}

impl Flow {
    fn priority_pending(&self, i: usize, pending: bool) {
        // Publish the count before the flag, so concurrent cancellation cannot subtract an
        // admission that has not reached the aggregate yet. Duplicate polls undo their credit.
        if pending {
            self.owner.light_pending[i].fetch_add(1, Ordering::Relaxed);
            if self.directions[i].priority_pending.swap(true, Ordering::AcqRel) { decrement_atomic_saturating(&self.owner.light_pending[i], 1); }
        } else if self.directions[i].priority_pending.swap(false, Ordering::AcqRel) {
            decrement_atomic_saturating(&self.owner.light_pending[i], 1);
        }
    }
    pub(super) fn reserve(self: &Arc<Self>, direction: RateDirection, requested: u64) -> AdaptiveReservation {
        self.reserve_at_epoch(direction, requested, current_epoch())
    }
    fn reserve_at_epoch(self: &Arc<Self>, direction: RateDirection, requested: u64, epoch: u64) -> AdaptiveReservation {
        let i = index(direction);
        let mut result = AdaptiveReservation { granted: requested, debits: [None, None, None, None, None], flow: Arc::clone(self), index: i, requested, settled: false };
        let config = self.owner.config.load();
        if !config.enabled || requested == 0 { return result; }
        self.directions[i].requested_at.store(epoch, Ordering::Relaxed);
        let previous_request = self.ip.directions[i].requested_at.swap(epoch, Ordering::Relaxed);
        let dormant = previous_request == 0 || epoch.saturating_sub(previous_request) > 100;
        let mut bulk = self.directions[i].bulk.load(Ordering::Relaxed);
        self.priority_pending(i, !bulk);
        self.directions[i].pending_bytes.store(requested.min(MAX_BORROW_CHUNK_BYTES), Ordering::Relaxed);
        let cap = bytes_per_epoch(if i == 0 { config.up_bps } else { config.down_bps });
        let d = &self.ip.directions[i];
        let startup = dormant || d.total_cap.load(Ordering::Relaxed) <= 1;
        let mut budget = ReserveCasBudget::new();
        // Epoch demand admission counts an IP once, so idle sockets cannot dilute a resumed flow.
        if !d.demand.ensure_active_weighted(epoch, &self.owner.demand[i], &mut budget, d.weight.load(Ordering::Relaxed)).unwrap_or(false) {
            result.granted = 0; self.waiting_until[i].store(epoch + 50, Ordering::Relaxed);
            self.owner.throttles.fetch_add(1, Ordering::Relaxed);
            if !bulk { self.owner.light_requested[i].store(epoch, Ordering::Relaxed); }
            return result;
        }
        let demand_state = self.owner.demand[i].state.load(Ordering::Relaxed);
        let credited_weight = d.demand.used.state.load(Ordering::Relaxed) & PACKED_USAGE_MASK;
        let fair = (cap.saturating_mul(credited_weight) / (demand_state & PACKED_USAGE_MASK).max(1)).max(1);
        let contended_bulk = bytes_per_epoch(config.contended_bulk_bps(i == 0)).min(cap);
        let priority = cap.saturating_sub(contended_bulk).max(1);
        let mut priority_fair = 0;
        if !bulk {
            // Publish original small-flow demand before overflow classification, including an IP
            // whose last control tick saw only bulk flows. Reclamation needs no one-second tick.
            self.owner.light_requested[i].store(epoch, Ordering::Relaxed);
            if !d.light_demand.ensure_active_weighted(epoch, &self.owner.light_demand[i], &mut budget, d.weight.load(Ordering::Relaxed)).unwrap_or(false) {
                result.granted = 0;
                self.waiting_until[i].store(epoch + 50, Ordering::Relaxed);
                self.owner.throttles.fetch_add(1, Ordering::Relaxed);
                return result;
            }
            let credited = d.light_demand.used.state.load(Ordering::Relaxed) & PACKED_USAGE_MASK;
            let weights = self.owner.light_demand[i].state.load(Ordering::Relaxed) & PACKED_USAGE_MASK;
            priority_fair = (priority.saturating_mul(credited) / weights.max(1)).max(1);

        }
        // Bytes already sent cannot be recalled. A pending small flow limits bulk on the next
        // 20 ms epoch; the short hold prevents packet gaps from repeatedly releasing its reserve.
        let recent_light = self.owner.light_requested[i].load(Ordering::Relaxed);
        let contended = self.owner.light_pending[i].load(Ordering::Relaxed) > 0 && recent_light != 0 && epoch.saturating_sub(recent_light) <= 10;
        let light_epoch = self.owner.light_demand[i].state.load(Ordering::Relaxed) >> PACKED_USAGE_BITS;
        // Below two bytes per epoch both classes cannot have a positive quantum. Once an actual
        // small IP has requested service, give its byte precedence over speculative borrowing.
        let bulk_cap = if contended && cap == 1 && light_epoch != 0 && epoch.saturating_sub(light_epoch) <= 10 { 0 }
            else if contended { contended_bulk } else { cap };
        let priority_credit = d.light_demand.used.state.load(Ordering::Relaxed);
        if priority_credit >> PACKED_USAGE_BITS == epoch {
            let weights = self.owner.light_demand[i].state.load(Ordering::Relaxed) & PACKED_USAGE_MASK;
            priority_fair = (priority.saturating_mul(priority_credit & PACKED_USAGE_MASK) / weights.max(1)).max(1);
        }
        let calculated = !startup && self.ip.allocation_version.load(Ordering::Acquire) == self.owner.admission_version.load(Ordering::Acquire);
        let class_calculated = !startup && self.ip.allocation_version.load(Ordering::Acquire) != 0;
        let light_allocation = d.light_cap.load(Ordering::Relaxed);
        if class_calculated && priority_fair > 0 && light_allocation > 0 { priority_fair = light_allocation; }
        let allocated = if calculated {
            d.total_cap.load(Ordering::Relaxed)
        } else { fair };
        let total_cap = if priority_fair > 0 { allocated.max(priority_fair.saturating_add(fair.min(contended_bulk)).min(cap)) } else if !contended { allocated.max(fair) } else { allocated };
        if !bulk {
            let used = d.light_used.state.load(Ordering::Relaxed);
            let global_used = self.owner.light[i].state.load(Ordering::Relaxed);
            bulk = (used >> PACKED_USAGE_BITS == epoch && used & PACKED_USAGE_MASK >= priority_fair)
                || (global_used >> PACKED_USAGE_BITS == epoch && global_used & PACKED_USAGE_MASK >= priority);
        }
        let bulk_allocation = d.bulk_cap.load(Ordering::Relaxed);
        let ip_bulk = if contended && class_calculated && bulk_allocation > 0 { bulk_allocation } else { total_cap };
        let (ip_bucket, ip_cap) = if bulk { (&d.bulk_used, ip_bulk) } else { (&d.light_used, priority_fair) };
        for (slot, bucket, ceiling) in [
            (0, ip_bucket, ip_cap), (1, &self.owner.global[i], cap),
            (2, &d.total_used, total_cap),
            (3, &self.owner.bulk[i], if bulk { bulk_cap } else { cap }),
            (4, &self.owner.light[i], priority),
        ] {
            if (slot == 3 && !bulk) || (slot == 4 && bulk) { continue; }
            match bucket.try_reserve_at(epoch, ceiling, result.granted.min(MAX_BORROW_CHUNK_BYTES), &mut budget) {
                Ok(Some(debit)) => { result.granted = debit.refundable; result.debits[slot] = Some(debit); }
                _ => { result.granted = 0; break; }
            }
        }
        for debit in result.debits.iter_mut().flatten() { let _ = debit.shrink_to(result.granted); }
        if result.granted == 0 {
            if !bulk { self.owner.light_requested[i].store(epoch, Ordering::Relaxed); }
            self.owner.throttles.fetch_add(1, Ordering::Relaxed);
            self.waiting_until[i].store(epoch + 50, Ordering::Relaxed);
        } else { self.waiting_until[i].store(0, Ordering::Relaxed); }
        result
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        for i in 0..2 { self.priority_pending(i, false); }
        if self.ip.flows.fetch_sub(1, Ordering::Relaxed) == 1 {
            decrement_atomic_saturating(&self.owner.active_ips, 1);
            self.owner.admission_version.fetch_add(1, Ordering::Release);
        }
    }
}

impl AdaptiveReservation {
    pub(super) fn idle_read(&self) {
        self.flow.priority_pending(self.index, false);
        self.flow.directions[self.index].requested_at.store(0, Ordering::Relaxed);
        self.flow.directions[self.index].pending_bytes.store(0, Ordering::Relaxed);
        self.flow.waiting_until[self.index].store(0, Ordering::Relaxed);
    }
    pub(super) fn settle(&mut self, committed: u64) {
        if self.settled { return; }
        self.settled = true;
        let bytes = committed.min(self.granted);
        if bytes < self.granted || bytes == self.requested { self.flow.priority_pending(self.index, false); }
        for debit in self.debits.iter_mut().flatten() { debit.settle(bytes); }
        self.flow.directions[self.index].pending_bytes.store(if bytes == self.granted { self.requested.saturating_sub(bytes).min(MAX_BORROW_CHUNK_BYTES) } else { 0 }, Ordering::Relaxed);
        if bytes > 0 {
            self.flow.directions[self.index].committed.fetch_add(bytes, Ordering::Relaxed);
            self.flow.ip.directions[self.index].committed.fetch_add(bytes, Ordering::Relaxed);
            if !self.flow.directions[self.index].bulk.load(Ordering::Relaxed) { self.flow.ip.directions[self.index].light_chunk.fetch_max(bytes, Ordering::Relaxed); }
            self.flow.ip.last_io.store(second(), Ordering::Relaxed);
        }
    }
}

impl Drop for AdaptiveReservation {
    fn drop(&mut self) {
        if !self.settled && self.granted > 0 { self.flow.priority_pending(self.index, false); }
    }
}

impl TrafficLimiter {
    /// Publishes a generation-fenced adaptive policy independently of static lease bindings.
    pub(crate) fn apply_shaping_from_source(&self, generation: u64, config: ShapingConfig) -> bool { self.adaptive.apply(generation, config) }
    /// Advances minute windows and publishes effective rates on the control plane.
    pub(crate) fn tick_shaping(&self) { self.adaptive.tick(); }
    /// Returns a bounded snapshot; this is never used by the data plane.
    pub(crate) fn shaping_snapshot(&self, offset: usize, limit: usize) -> ShapingSnapshot { self.adaptive.snapshot(offset, limit) }
}

pub(super) struct ShapingActivity { counter: Arc<AtomicU64>, epoch: tokio::time::Instant }

impl TrafficLease {
    /// Keeps frame idle accounting aware of pacing below buffered TLS framing.
    pub(crate) fn set_shaping_activity(&self, counter: Arc<AtomicU64>, epoch: tokio::time::Instant) { self.shaping_activity.store(Some(Arc::new(ShapingActivity { counter, epoch }))); }
    pub(super) fn adaptive_flow(&self) -> Option<Arc<Flow>> {
        if !self.limiter.adaptive.enabled() { return None; }
        if let Some(flow) = self.adaptive.load_full() { return Some(flow); }
        let _guard = self.refresh.lock();
        if let Some(flow) = self.adaptive.load_full() { return Some(flow); }
        let flow = self.limiter.adaptive.acquire(self.client_ip)?;
        self.adaptive.store(Some(Arc::clone(&flow)));
        Some(flow)
    }
    /// Cumulative pacing time excludes peer waits and is used to preserve frame idle deadlines.
    pub(crate) fn shaping_wait_ms(&self, direction: RateDirection) -> u64 {
        self.adaptive.load().as_ref().map(|flow| flow.wait_ms[index(direction)].load(Ordering::Relaxed)).unwrap_or(0)
    }
    /// Reports a recent budget wait in exactly one direction.
    pub(crate) fn shaping_waiting_direction(&self, direction: RateDirection) -> bool {
        self.limiter.adaptive.enabled() && self.adaptive.load().as_ref().is_some_and(|flow| flow.waiting_until[index(direction)].load(Ordering::Relaxed) > current_epoch())
    }
    /// True only while recently polled I/O is awaiting adaptive budget, not a peer read.
    pub(crate) fn shaping_waiting(&self) -> bool {
        self.limiter.adaptive.enabled() && self.adaptive.load().as_ref().is_some_and(|f| f.waiting_until.iter().any(|value| value.load(Ordering::Relaxed) > current_epoch()))
    }
}

#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod class_tests;
