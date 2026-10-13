use super::*;

/// Fixed memory rolling history sampled only by the control-plane tick.
pub(crate) struct Window {
    samples: [(u64, u64); WINDOW],
    last_total: u64,
    last_second: u64,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            samples: [(0, 0); WINDOW],
            last_total: 0,
            last_second: 0,
        }
    }
}

impl Window {
    pub(crate) fn sample(&mut self, now: u64, total: u64) -> (u64, u64) {
        let delta = total.saturating_sub(self.last_total);
        self.last_total = total;
        if now != self.last_second {
            self.samples[now as usize % WINDOW] = (now, delta);
            self.last_second = now;
        } else {
            self.samples[now as usize % WINDOW].1 += delta;
        }
        let sum = self
            .samples
            .iter()
            .filter(|(at, _)| now.saturating_sub(*at) < WINDOW as u64 && *at > 0)
            .map(|(_, bytes)| bytes)
            .sum();
        (sum, delta)
    }
}

/// Weighted water filling; integer leftovers are deterministic and total grants never exceed budget.
pub(crate) fn allocate(budget: u64, demand: &[(f64, u64)]) -> Vec<u64> {
    let mut grants = vec![0; demand.len()];
    let mut pending: Vec<usize> = (0..demand.len())
        .filter(|i| demand[*i].1 > 0 && demand[*i].0.is_finite() && demand[*i].0 > 0.0)
        .collect();
    pending.sort_unstable_by(|a, b| {
        (demand[*a].1 as f64 / demand[*a].0)
            .total_cmp(&(demand[*b].1 as f64 / demand[*b].0))
            .then(a.cmp(b))
    });
    let mut remaining = budget;
    let mut weight: f64 = pending.iter().map(|i| demand[*i].0).sum();
    let mut first = 0;
    while first < pending.len() && weight > 0.0 {
        let i = pending[first];
        if demand[i].1 as f64 > remaining as f64 * demand[i].0 / weight {
            break;
        }
        let grant = demand[i].1.min(remaining);
        grants[i] = grant;
        remaining -= grant;
        weight -= demand[i].0;
        first += 1;
    }
    if weight > 0.0 {
        let total = remaining;
        for i in &pending[first..] {
            let grant = ((total as f64 * demand[*i].0 / weight) as u64)
                .min(remaining)
                .min(demand[*i].1);
            grants[*i] = grant;
            remaining -= grant;
        }
        for i in &pending[first..] {
            if remaining == 0 {
                break;
            }
            if grants[*i] < demand[*i].1 {
                grants[*i] += 1;
                remaining -= 1;
            }
        }
    }
    grants
}

pub(super) fn cleanup(registry: &mut Registry, now: u64) {
    registry.flows = 0;
    let mut retired = [0u64; 2];
    registry.ips.retain(|_, entry| {
        entry.flows.retain(|(flow, _)| flow.strong_count() > 0);
        registry.flows += entry.flows.len();
        let keep = !entry.flows.is_empty()
            || now.saturating_sub(entry.state.last_io.load(Ordering::Relaxed)) < WINDOW as u64;
        if !keep {
            for (i, total) in retired.iter_mut().enumerate() {
                *total = total
                    .saturating_add(entry.state.directions[i].committed.load(Ordering::Relaxed));
            }
        }
        keep
    });
    for (i, total) in retired.iter().enumerate() {
        registry.retired[i] = registry.retired[i].saturating_add(*total);
    }
}

pub(super) fn calculate(registry: &mut Registry, now: u64) {
    let epoch = current_epoch();
    let mut entries: Vec<_> = registry.ips.iter_mut().collect();
    entries.sort_unstable_by_key(|(ip, _)| **ip);
    for (_, entry) in &mut entries {
        for i in 0..2 {
            let d = &entry.state.directions[i];
            let (window, delta) = entry.history[i].sample(now, d.committed.load(Ordering::Relaxed));
            // Keep budget pressure for the complete interval, including disconnected flows.
            let waiting = d.budget_waited.swap(false, Ordering::Relaxed)
                | entry
                    .flows
                    .iter()
                    .filter_map(|(flow, _)| flow.upgrade())
                    .any(|flow| flow.waiting_until[i].load(Ordering::Relaxed) > epoch);
            entry.pressure[i].sample(&registry.config, now, delta, window, waiting);
            entry.pressure[i].publish(d);
            d.window.store(window, Ordering::Relaxed);
            if registry.tick != now {
                d.speed.store(delta.saturating_mul(8), Ordering::Relaxed);
            }
            for (flow, history) in &mut entry.flows {
                if let Some(flow) = flow.upgrade() {
                    let d = &flow.directions[i];
                    if epoch.saturating_sub(d.requested_at.load(Ordering::Relaxed)) > 10 {
                        flow.priority_pending(i, false);
                    }
                    let (window, delta) =
                        history[i].sample(now, d.committed.load(Ordering::Relaxed));
                    if registry.tick != now {
                        d.speed.store(delta.saturating_mul(8), Ordering::Relaxed);
                    }
                    d.window.store(window, Ordering::Relaxed);
                    let bulk = d.bulk.load(Ordering::Relaxed);
                    if !bulk && window >= registry.config.bulk_enter_bytes {
                        d.bulk.store(true, Ordering::Relaxed);
                    }
                    if bulk && window < registry.config.bulk_exit_bytes {
                        d.bulk.store(false, Ordering::Relaxed);
                    }
                }
            }
        }
    }
    for i in 0..2 {
        let capacity = bytes_per_epoch(if i == 0 {
            registry.config.up_bps
        } else {
            registry.config.down_bps
        });
        let active = entries
            .iter()
            .filter(|(_, e)| {
                e.flows.iter().filter_map(|(f, _)| f.upgrade()).any(|f| {
                    epoch.saturating_sub(f.directions[i].requested_at.load(Ordering::Relaxed))
                        <= 100
                })
            })
            .count()
            .max(1);
        let fair_minute = (capacity.saturating_mul(50 * WINDOW as u64) / active as u64).max(1);
        let mut light = Vec::new();
        let mut bulk = Vec::new();
        let mut needs = Vec::new();
        for (_, entry) in &entries {
            let mut has_light = false;
            let mut has_bulk = false;
            for (flow, _) in &entry.flows {
                if let Some(flow) = flow.upgrade() {
                    if epoch.saturating_sub(flow.directions[i].requested_at.load(Ordering::Relaxed))
                        <= 100
                    {
                        if flow.is_bulk(i, &registry.config) {
                            has_bulk = true;
                        } else {
                            has_light = true;
                        }
                    }
                }
            }
            let pending =
                entry
                    .flows
                    .iter()
                    .filter_map(|(f, _)| f.upgrade())
                    .fold(0u64, |sum, flow| {
                        sum.saturating_add(flow.directions[i].pending_bytes.load(Ordering::Relaxed))
                    });
            let desired = (entry.state.directions[i].speed.load(Ordering::Relaxed) / 8 / 50)
                .saturating_mul(2)
                .saturating_add(
                    entry.state.directions[i]
                        .light_chunk
                        .swap(0, Ordering::Relaxed),
                )
                .saturating_add(pending)
                .max(64)
                .min(capacity);
            let weight = (fair_minute as f64
                / entry.state.directions[i]
                    .window
                    .load(Ordering::Relaxed)
                    .max(fair_minute) as f64)
                .clamp(1.0 / 16.0, 1.0);
            entry.state.directions[i]
                .weight
                .store((weight * 16.0).ceil() as u64, Ordering::Relaxed);
            light.push((weight, if has_light { desired } else { 0 }));
            needs.push(if has_bulk {
                capacity
            } else if has_light {
                desired
            } else {
                0
            });
            bulk.push((weight, if has_bulk || has_light { capacity } else { 0 }));
        }
        let both = light.iter().any(|(_, d)| *d > 0) && bulk.iter().any(|(_, d)| *d > 0);
        let light_budget = if both {
            capacity
                .saturating_sub(
                    bytes_per_epoch(registry.config.contended_bulk_bps(i == 0)).min(capacity),
                )
                .max(1)
        } else {
            capacity
        };
        let light_rates = allocate(light_budget, &light);
        for (i, (_, demand)) in bulk.iter_mut().enumerate() {
            *demand = needs[i].saturating_sub(light_rates[i]);
        }
        let bulk_rates = allocate(
            capacity
                .saturating_sub(light_rates.iter().sum())
                .min(bytes_per_epoch(registry.config.idle_bulk_bps(i == 0))),
            &bulk,
        );
        for (idx, (_, entry)) in entries.iter().enumerate() {
            let d = &entry.state.directions[i];
            // Small-flow capacity is reserved even for an idle IP, avoiding a one-second startup stall.
            let total = light_rates[idx].saturating_add(bulk_rates[idx]);
            d.total_cap.store(total.max(1), Ordering::Release);
            d.light_cap.store(light_rates[idx], Ordering::Release);
            d.bulk_cap.store(bulk_rates[idx], Ordering::Release);
        }
    }
}
