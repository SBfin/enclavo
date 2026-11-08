//! Route health (latency average and circuit breaker) and per-provider concurrency slots.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    catalog::{Catalog, Route},
    config::RouterConfig,
};

#[derive(Debug, Default)]
struct Health {
    ewma_ms: Option<f64>,
    consecutive_failures: u32,
    trips: u32,
    open_until: Option<Instant>,
    /// A half-open circuit lets exactly one probe through at a time.
    probing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct Router {
    cfg: RouterConfig,
    health: HashMap<String, Mutex<Health>>,
    slots: HashMap<String, Arc<Semaphore>>,
    live_inflight: Arc<AtomicUsize>,
}

/// Held while a request runs on a route: the provider slot and, if it is a probe, the probe flag.
pub struct Lease {
    router: Arc<Router>,
    route_id: String,
    probe: bool,
    _slot: OwnedSemaphorePermit,
    started: Instant,
    reported: bool,
}

/// Counts a live request for as long as it is alive, so batch work can yield to live traffic.
pub struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Router {
    pub fn new(cfg: RouterConfig, catalog: &Catalog) -> Self {
        let health = catalog.routes.iter().map(|r| (r.id.clone(), Mutex::new(Health::default()))).collect();
        let slots =
            catalog.providers.values().map(|p| (p.id.clone(), Arc::new(Semaphore::new(p.max_concurrency)))).collect();
        Self { cfg, health, slots, live_inflight: Arc::new(AtomicUsize::new(0)) }
    }

    pub fn config(&self) -> &RouterConfig {
        &self.cfg
    }

    pub fn live_request(&self) -> LiveGuard {
        self.live_inflight.fetch_add(1, Ordering::SeqCst);
        LiveGuard(self.live_inflight.clone())
    }

    pub fn live_inflight(&self) -> usize {
        self.live_inflight.load(Ordering::SeqCst)
    }

    /// Observed time to first byte; routes never measured sort as average.
    pub fn latency_ms(&self, route: &Route) -> f64 {
        self.health.get(&route.id).and_then(|h| h.lock().ewma_ms).unwrap_or(1_000.0)
    }

    pub fn circuit(&self, route_id: &str) -> CircuitState {
        let Some(h) = self.health.get(route_id) else { return CircuitState::Closed };
        let h = h.lock();
        match h.open_until {
            None => CircuitState::Closed,
            Some(until) if Instant::now() < until => CircuitState::Open,
            Some(_) => CircuitState::HalfOpen,
        }
    }

    /// Reserves a route if its circuit allows it and its provider has a free slot.
    pub fn try_lease(self: &Arc<Self>, route: &Route) -> Option<Lease> {
        let health = self.health.get(&route.id)?;
        let slot = self.slots.get(&route.provider.id)?.clone().try_acquire_owned().ok()?;
        let mut h = health.lock();
        let probe = match h.open_until {
            None => false,
            Some(until) if Instant::now() < until => return None,
            Some(_) if h.probing => return None,
            Some(_) => {
                h.probing = true;
                true
            }
        };
        Some(Lease {
            router: self.clone(),
            route_id: route.id.clone(),
            probe,
            _slot: slot,
            started: Instant::now(),
            reported: false,
        })
    }

    fn record_success(&self, route_id: &str, latency: Duration) {
        let Some(health) = self.health.get(route_id) else { return };
        let mut h = health.lock();
        let ms = latency.as_secs_f64() * 1_000.0;
        h.ewma_ms = Some(match h.ewma_ms {
            Some(prev) => prev + self.cfg.ewma_alpha * (ms - prev),
            None => ms,
        });
        h.consecutive_failures = 0;
        h.trips = 0;
        h.open_until = None;
        h.probing = false;
    }

    fn record_failure(&self, route_id: &str, retry_after: Option<Duration>) {
        let Some(health) = self.health.get(route_id) else { return };
        let mut h = health.lock();
        h.consecutive_failures += 1;
        let was_probe = h.probing;
        h.probing = false;
        // Requests that were already in flight when the circuit opened report their failures
        // late; they say nothing new, so they must not stretch the cooldown.
        let already_open = h.open_until.is_some_and(|until| Instant::now() < until);
        if already_open && !was_probe {
            return;
        }
        if retry_after.is_some() || was_probe || h.consecutive_failures >= self.cfg.failure_threshold {
            let base = Duration::from_secs(self.cfg.cooldown_secs);
            let backoff = base.saturating_mul(2u32.saturating_pow(h.trips.min(16)));
            let cooldown = retry_after.unwrap_or(backoff).min(Duration::from_secs(self.cfg.max_cooldown_secs));
            h.trips += 1;
            h.open_until = Some(Instant::now() + cooldown);
            tracing::warn!(route = route_id, cooldown_ms = cooldown.as_millis() as u64, "route circuit opened");
            metrics::counter!("enclavo_circuit_open_total", "route" => route_id.to_string()).increment(1);
        }
    }

    fn release_probe(&self, route_id: &str) {
        if let Some(h) = self.health.get(route_id) {
            h.lock().probing = false;
        }
    }
}

impl Lease {
    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    /// The route answered: records time to first byte.
    pub fn succeed(&mut self) {
        self.reported = true;
        self.router.record_success(&self.route_id, self.started.elapsed());
    }

    /// The route failed in a way that says something about its health.
    pub fn fail(&mut self, retry_after: Option<Duration>) {
        self.reported = true;
        self.router.record_failure(&self.route_id, retry_after);
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A probe that ended without a verdict (e.g. the client went away) must not block the route.
        if self.probe && !self.reported {
            self.router.release_probe(&self.route_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Provider;

    fn setup(max_concurrency: usize) -> (Arc<Router>, Arc<Route>) {
        setup_with(max_concurrency, 0)
    }

    fn setup_with(max_concurrency: usize, cooldown_secs: u64) -> (Arc<Router>, Arc<Route>) {
        let provider = Arc::new(Provider {
            id: "p".into(),
            base_url: String::new(),
            api_key: None,
            max_concurrency,
            timeout: Duration::from_secs(1),
        });
        let route = Arc::new(Route {
            id: "r".into(),
            model: "m".into(),
            display_name: "m".into(),
            provider: provider.clone(),
            upstream_model: "m".into(),
            attestation: String::new(),
            attested: true,
            score: 1,
            tokens_per_iu: 1,
            micro_usd_per_iu: 1,
            max_output_tokens: 1,
            prompt_weight_bps: 10_000,
        });
        let catalog = Catalog { providers: HashMap::from([("p".into(), provider)]), routes: vec![route.clone()] };
        let cfg = RouterConfig { failure_threshold: 2, cooldown_secs, ..Default::default() };
        (Arc::new(Router::new(cfg, &catalog)), route)
    }

    #[test]
    fn opens_after_threshold_and_allows_one_probe() {
        let (router, route) = setup(4);
        router.try_lease(&route).unwrap().fail(None);
        assert_eq!(router.circuit("r"), CircuitState::Closed);
        router.try_lease(&route).unwrap().fail(None);
        // cooldown is zero, so the circuit is immediately half-open
        assert_eq!(router.circuit("r"), CircuitState::HalfOpen);
        let mut probe = router.try_lease(&route).expect("first probe");
        assert!(router.try_lease(&route).is_none(), "only one probe at a time");
        probe.succeed();
        drop(probe);
        assert_eq!(router.circuit("r"), CircuitState::Closed);
    }

    #[test]
    fn retry_after_opens_immediately() {
        let (router, route) = setup(4);
        router.try_lease(&route).unwrap().fail(Some(Duration::from_secs(60)));
        assert_eq!(router.circuit("r"), CircuitState::Open);
        assert!(router.try_lease(&route).is_none());
    }

    #[test]
    fn provider_slots_limit_concurrency() {
        let (router, route) = setup(1);
        let held = router.try_lease(&route).unwrap();
        assert!(router.try_lease(&route).is_none());
        drop(held);
        assert!(router.try_lease(&route).is_some());
    }

    #[test]
    fn concurrent_failures_trip_the_circuit_once() {
        let (router, route) = setup_with(8, 60);
        // Four requests take the route while it is healthy, then all fail together.
        let mut leases: Vec<_> = (0..4).map(|_| router.try_lease(&route).unwrap()).collect();
        for lease in &mut leases {
            lease.fail(None);
        }
        assert_eq!(router.health["r"].lock().trips, 1, "one outage, one trip");
        assert_eq!(router.circuit("r"), CircuitState::Open);
    }

    #[test]
    fn abandoned_probe_is_released() {
        let (router, route) = setup(4);
        router.try_lease(&route).unwrap().fail(None);
        router.try_lease(&route).unwrap().fail(None);
        drop(router.try_lease(&route).unwrap());
        assert!(router.try_lease(&route).is_some());
    }
}
