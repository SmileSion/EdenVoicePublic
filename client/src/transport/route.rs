use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteKind {
    LanDirect,
    Ipv6Direct,
    P2PDirect,
    Sfu,
    Relay,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteState {
    Discovering,
    Checking,
    Connected,
    Qualified,
    Degraded,
    Failed,
    Closed,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteMetrics {
    pub rtt_ms: f64,
    pub packet_loss_percent: f64,
    pub jitter_ms: f64,
    pub packets_sent: u64,
    pub packets_received: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct RouteThresholds {
    pub rtt_ms: f64,
    pub loss_percent: f64,
    pub jitter_ms: f64,
}
#[derive(Clone, Debug)]
pub struct Route {
    pub peer_id: u64,
    pub kind: RouteKind,
    pub endpoint: SocketAddr,
    pub state: RouteState,
    pub metrics: RouteMetrics,
    pub stable_since: Option<Instant>,
    degraded_since: Option<Instant>,
    last_observed: Option<Instant>,
    loss_history: VecDeque<(Instant, u64, u64)>,
}
// Qualified paths are probed every 200 ms. Two missed probe intervals
// warrant a temporary SFU copy, without revoking the route qualification.
impl Route {
    pub(super) fn needs_sfu_protection(&self, now: Instant) -> bool {
        let grace_ms = (200.0 + self.metrics.rtt_ms * 2.0 + self.metrics.jitter_ms * 4.0)
            .clamp(400.0, 1_000.0);
        self.last_observed.is_none_or(|at| {
            now.saturating_duration_since(at) >= Duration::from_secs_f64(grace_ms / 1_000.0)
        })
    }
}

pub struct RouteManager {
    routes: BTreeMap<(u64, SocketAddr), Route>,
    qualification: Duration,
    max_rtt_ms: f64,
    max_loss: f64,
    max_jitter_ms: f64,
    degrade_rtt_ms: f64,
    degrade_loss: f64,
    degrade_jitter_ms: f64,
    degradation_duration: Duration,
}
impl RouteManager {
    pub fn new(qualification: Duration) -> Self {
        Self {
            routes: BTreeMap::new(),
            qualification,
            max_rtt_ms: 80.0,
            max_loss: 2.0,
            max_jitter_ms: 15.0,
            degrade_rtt_ms: 250.0,
            degrade_loss: 5.0,
            degrade_jitter_ms: 40.0,
            degradation_duration: Duration::from_millis(1500),
        }
    }
    pub fn add_candidate(&mut self, peer_id: u64, kind: RouteKind, endpoint: SocketAddr) {
        self.routes.entry((peer_id, endpoint)).or_insert(Route {
            peer_id,
            kind,
            endpoint,
            state: RouteState::Checking,
            metrics: RouteMetrics::default(),
            stable_since: None,
            degraded_since: None,
            last_observed: None,
            loss_history: VecDeque::new(),
        });
    }
    pub fn configure(
        &mut self,
        qualification: Duration,
        qualification_thresholds: RouteThresholds,
        degradation_thresholds: RouteThresholds,
    ) {
        self.qualification = qualification;
        self.max_rtt_ms = qualification_thresholds.rtt_ms;
        self.max_loss = qualification_thresholds.loss_percent;
        self.max_jitter_ms = qualification_thresholds.jitter_ms;
        self.degrade_rtt_ms = degradation_thresholds.rtt_ms;
        self.degrade_loss = degradation_thresholds.loss_percent;
        self.degrade_jitter_ms = degradation_thresholds.jitter_ms;
    }
    pub fn observe(
        &mut self,
        peer_id: u64,
        endpoint: SocketAddr,
        sample: RouteMetrics,
        now: Instant,
    ) -> bool {
        let Some(route) = self.routes.get_mut(&(peer_id, endpoint)) else {
            return false;
        };
        route.metrics = sample;
        route.last_observed = Some(now);
        if route
            .loss_history
            .back()
            .is_some_and(|(_, sent, received)| {
                sample.packets_sent < *sent || sample.packets_received < *received
            })
        {
            route.loss_history.clear();
        }
        while route
            .loss_history
            .front()
            .is_some_and(|(at, _, _)| now.saturating_duration_since(*at) > Duration::from_secs(8))
            || route.loss_history.len() >= 128
        {
            route.loss_history.pop_front();
        }
        let sustained_loss = route
            .loss_history
            .front()
            .is_some_and(|(_, sent, received)| {
                let sent = sample.packets_sent.saturating_sub(*sent);
                let received = sample.packets_received.saturating_sub(*received);
                let missing = sent.saturating_sub(received);
                // One in-flight/delayed ACK in a sparse probe stream is not loss.
                sent >= 10
                    && missing >= 2
                    && 100.0 * missing as f64 / sent as f64 >= self.degrade_loss
            });
        route
            .loss_history
            .push_back((now, sample.packets_sent, sample.packets_received));
        let (_, sent_before, received_before) = route.loss_history.front().copied().unwrap();
        let sent = sample.packets_sent.saturating_sub(sent_before);
        let received = sample.packets_received.saturating_sub(received_before);
        route.metrics.packet_loss_percent = if sent == 0 {
            0.0
        } else {
            100.0 * sent.saturating_sub(received) as f64 / sent as f64
        };
        let qualifies = sample.rtt_ms < self.max_rtt_ms
            && route.metrics.packet_loss_percent < self.max_loss
            && sample.jitter_ms < self.max_jitter_ms;
        // Probe loss is measured from a very small, cumulative sample while a
        // direct route is established (normally one probe every 500 ms). A
        // single delayed ACK could therefore make the calculated loss jump to
        // 20% and immediately tear down an otherwise healthy route. Once a
        // route is qualified, sustained missing ACKs are detected by the
        // degrade timeout in AdaptiveTransport; an ACK that does arrive still
        // has to meet the wider RTT/jitter limits here.
        let survives = sample.rtt_ms < self.degrade_rtt_ms
            && sample.jitter_ms < self.degrade_jitter_ms
            && !sustained_loss;
        if route.state == RouteState::Qualified {
            if survives {
                route.degraded_since = None;
                return true;
            }
            let since = *route.degraded_since.get_or_insert(now);
            if now.saturating_duration_since(since) < self.degradation_duration {
                return true;
            }
        }
        route.degraded_since = None;
        // A route that was healthy for hours must still leave P2P when its
        // recent probe window is bad, even if lifetime loss remains tiny.
        let qualifies = qualifies && !sustained_loss;
        if qualifies {
            let since = *route.stable_since.get_or_insert(now);
            route.state = if now.duration_since(since) >= self.qualification {
                RouteState::Qualified
            } else {
                RouteState::Connected
            };
        } else {
            route.stable_since = None;
            route.state = RouteState::Degraded;
        }
        route.state == RouteState::Qualified
    }
    pub fn best(&self, peer_id: u64, require_qualified: bool) -> Option<&Route> {
        self.routes
            .values()
            .filter(|r| {
                r.peer_id == peer_id
                    && (!require_qualified
                        || (r.state == RouteState::Qualified
                            && r.last_observed.is_some_and(|at| {
                                Instant::now().saturating_duration_since(at)
                                    < self.degradation_duration
                            })))
            })
            .max_by(|a, b| score(a).total_cmp(&score(b)))
    }
    pub fn configure_degradation_duration(&mut self, duration: Duration) {
        self.degradation_duration = duration;
    }
    pub fn peers(&self) -> Vec<u64> {
        let mut x: Vec<_> = self.routes.values().map(|r| r.peer_id).collect();
        x.sort_unstable();
        x.dedup();
        x
    }
    pub fn metrics_for(&self, peer_id: u64, endpoint: SocketAddr) -> Option<RouteMetrics> {
        self.routes
            .get(&(peer_id, endpoint))
            .map(|route| route.metrics)
    }
    pub(super) fn fresh_metrics_for(
        &self,
        peer_id: u64,
        endpoint: SocketAddr,
        now: Instant,
    ) -> Option<(RouteMetrics, Instant)> {
        let route = self.routes.get(&(peer_id, endpoint))?;
        let observed_at = route.last_observed?;
        (now.saturating_duration_since(observed_at) < self.degradation_duration)
            .then_some((route.metrics, observed_at))
    }
    pub fn has_route(&self, peer_id: u64, endpoint: SocketAddr) -> bool {
        self.routes.contains_key(&(peer_id, endpoint))
    }
    pub fn accepts_media_from(&self, peer_id: u64, endpoint: SocketAddr) -> bool {
        self.routes.get(&(peer_id, endpoint)).is_some_and(|route| {
            matches!(route.state, RouteState::Connected | RouteState::Qualified)
        })
    }
    pub fn retain_peers(&mut self, peers: &[u64]) {
        self.routes
            .retain(|(peer_id, _), _| peers.contains(peer_id));
    }
    pub fn remove_peer(&mut self, peer_id: u64) {
        self.routes
            .retain(|(route_peer_id, _), _| *route_peer_id != peer_id);
    }
    pub fn degrade_peer(&mut self, peer_id: u64) {
        for route in self
            .routes
            .values_mut()
            .filter(|route| route.peer_id == peer_id)
        {
            route.state = RouteState::Degraded;
            route.stable_since = None;
            route.degraded_since = None;
        }
    }
    pub fn degrade_route(&mut self, peer_id: u64, endpoint: SocketAddr) {
        if let Some(route) = self.routes.get_mut(&(peer_id, endpoint)) {
            route.state = RouteState::Degraded;
            route.stable_since = None;
            route.degraded_since = None;
        }
    }
}
fn score(route: &Route) -> f64 {
    let direct = match route.kind {
        RouteKind::LanDirect => 50.0,
        RouteKind::Ipv6Direct => 40.0,
        RouteKind::P2PDirect => 30.0,
        RouteKind::Sfu => 10.0,
        RouteKind::Relay => 0.0,
    };
    direct
        - route.metrics.rtt_ms * 0.2
        - route.metrics.packet_loss_percent * 8.0
        - route.metrics.jitter_ms * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(rtt_ms: f64, received: u64, sent: u64, jitter_ms: f64) -> RouteMetrics {
        RouteMetrics {
            rtt_ms,
            packets_received: received,
            packets_sent: sent,
            jitter_ms,
            ..Default::default()
        }
    }
    #[test]
    fn qualification_is_continuous_and_not_immediate() {
        let ep = "127.0.0.1:9".parse().unwrap();
        let start = Instant::now();
        let mut m = RouteManager::new(Duration::from_secs(20));
        m.add_candidate(2, RouteKind::LanDirect, ep);
        assert!(!m.observe(2, ep, sample(20.0, 1, 1, 1.0), start));
        assert!(!m.observe(
            2,
            ep,
            sample(20.0, 50, 50, 1.0),
            start + Duration::from_secs(19)
        ));
        assert!(m.observe(
            2,
            ep,
            sample(20.0, 51, 51, 1.0),
            start + Duration::from_secs(20)
        ));
    }
    #[test]
    fn bad_sample_resets_qualification() {
        let ep = "127.0.0.1:9".parse().unwrap();
        let start = Instant::now();
        let mut m = RouteManager::new(Duration::from_secs(2));
        m.add_candidate(2, RouteKind::P2PDirect, ep);
        m.observe(2, ep, sample(10.0, 1, 1, 1.0), start);
        m.observe(
            2,
            ep,
            sample(500.0, 2, 2, 1.0),
            start + Duration::from_secs(2),
        );
        assert!(!m.observe(
            2,
            ep,
            sample(10.0, 3, 3, 1.0),
            start + Duration::from_secs(3)
        ));
    }

    #[test]
    fn qualified_route_uses_degradation_hysteresis() {
        let endpoint = "127.0.0.1:9".parse().unwrap();
        let now = Instant::now();
        let mut m = RouteManager::new(Duration::ZERO);
        m.add_candidate(2, RouteKind::P2PDirect, endpoint);
        assert!(m.observe(2, endpoint, sample(20.0, 100, 100, 1.0), now));
        assert!(m.observe(
            2,
            endpoint,
            sample(120.0, 198, 200, 20.0),
            now + Duration::from_secs(1)
        ));
        assert!(m.observe(
            2,
            endpoint,
            sample(300.0, 199, 200, 20.0),
            now + Duration::from_secs(2)
        ));
        assert!(!m.observe(
            2,
            endpoint,
            sample(300.0, 200, 201, 20.0),
            now + Duration::from_secs(4)
        ));
    }

    #[test]
    fn one_latency_spike_does_not_migrate_a_qualified_route() {
        let ep = "127.0.0.1:9".parse().unwrap();
        let now = Instant::now();
        let mut manager = RouteManager::new(Duration::ZERO);
        manager.add_candidate(2, RouteKind::P2PDirect, ep);
        assert!(manager.observe(2, ep, sample(20.0, 1, 1, 1.0), now));
        assert!(manager.observe(
            2,
            ep,
            sample(300.0, 2, 2, 45.0),
            now + Duration::from_secs(1)
        ));
        assert!(manager.observe(2, ep, sample(20.0, 3, 3, 1.0), now + Duration::from_secs(2)));
        assert!(manager.observe(
            2,
            ep,
            sample(300.0, 4, 4, 45.0),
            now + Duration::from_secs(3)
        ));
    }

    #[test]
    fn recent_persistent_loss_is_not_hidden_by_a_long_healthy_history() {
        let ep = "127.0.0.1:9".parse().unwrap();
        let now = Instant::now();
        let mut m = RouteManager::new(Duration::ZERO);
        m.add_candidate(2, RouteKind::P2PDirect, ep);
        assert!(m.observe(2, ep, sample(20.0, 10000, 10000, 1.0), now));
        assert!(m.observe(
            2,
            ep,
            sample(20.0, 10006, 10010, 1.0),
            now + Duration::from_secs(5)
        ));
        assert!(!m.observe(
            2,
            ep,
            sample(20.0, 10008, 10014, 1.0),
            now + Duration::from_secs(7)
        ));
        assert!(m.best(2, true).is_none());
    }

    #[test]
    fn qualified_route_does_not_flap_on_one_missed_low_rate_probe() {
        let endpoint = "127.0.0.1:9".parse().unwrap();
        let now = Instant::now();
        let mut m = RouteManager::new(Duration::ZERO);
        m.add_candidate(2, RouteKind::P2PDirect, endpoint);
        assert!(m.observe(2, endpoint, sample(2.0, 4, 4, 0.2), now));
        // With probes sent every 500 ms, one delayed ACK used to appear as
        // 20% cumulative loss and immediately caused a route migration.
        assert!(m.observe(
            2,
            endpoint,
            sample(2.0, 4, 5, 0.2),
            now + Duration::from_millis(500)
        ));
    }

    #[test]
    fn degrading_one_candidate_keeps_another_qualified_candidate_available() {
        let first = "127.0.0.1:9".parse().unwrap();
        let second = "127.0.0.1:10".parse().unwrap();
        let now = Instant::now();
        let mut m = RouteManager::new(Duration::ZERO);
        m.add_candidate(2, RouteKind::LanDirect, first);
        m.add_candidate(2, RouteKind::LanDirect, second);
        assert!(m.observe(2, first, sample(2.0, 1, 1, 0.2), now));
        assert!(m.observe(2, second, sample(3.0, 1, 1, 0.2), now));
        m.degrade_route(2, first);
        assert_eq!(m.best(2, true).map(|route| route.endpoint), Some(second));
    }

    #[test]
    fn departed_peers_do_not_keep_stale_routes() {
        let mut m = RouteManager::new(Duration::ZERO);
        m.add_candidate(2, RouteKind::LanDirect, "127.0.0.1:9".parse().unwrap());
        m.add_candidate(3, RouteKind::LanDirect, "127.0.0.1:10".parse().unwrap());
        m.retain_peers(&[3]);
        assert_eq!(m.peers(), vec![3]);
    }

    #[test]
    fn candidate_generation_reset_removes_old_peer_endpoints() {
        let mut m = RouteManager::new(Duration::ZERO);
        let old = "127.0.0.1:9".parse().unwrap();
        m.add_candidate(2, RouteKind::P2PDirect, old);
        m.remove_peer(2);
        assert!(!m.has_route(2, old));
    }

    #[test]
    fn checking_candidates_cannot_send_media_before_probe_validation() {
        let endpoint = "127.0.0.1:9".parse().unwrap();
        let mut m = RouteManager::new(Duration::from_secs(1));
        m.add_candidate(2, RouteKind::P2PDirect, endpoint);
        assert!(!m.accepts_media_from(2, endpoint));
        m.observe(2, endpoint, sample(10.0, 1, 1, 0.0), Instant::now());
        assert!(m.accepts_media_from(2, endpoint));
        m.degrade_peer(2);
        assert!(!m.accepts_media_from(2, endpoint));
    }
}

#[cfg(test)]
mod recovery_regression_tests {
    use super::*;
    #[test]
    fn protection_tolerates_probe_cadence_and_rtt_without_waiting_for_failure() {
        let mut routes = RouteManager::new(Duration::ZERO);
        let endpoint = "127.0.0.1:12345".parse().unwrap();
        let now = Instant::now();
        routes.add_candidate(7, RouteKind::P2PDirect, endpoint);
        routes.observe(
            7,
            endpoint,
            RouteMetrics {
                rtt_ms: 10.0,
                ..Default::default()
            },
            now,
        );
        let route = routes.best(7, true).unwrap();
        assert!(!route.needs_sfu_protection(now + Duration::from_millis(399)));
        assert!(route.needs_sfu_protection(now + Duration::from_millis(400)));
        let mut slow = route.clone();
        slow.metrics.rtt_ms = 200.0;
        slow.metrics.jitter_ms = 20.0;
        assert!(!slow.needs_sfu_protection(now + Duration::from_millis(600)));
        assert!(slow.needs_sfu_protection(now + Duration::from_millis(680)));
    }

    #[test]
    fn expired_qualification_is_not_selected_between_probe_ticks() {
        let mut routes = RouteManager::new(Duration::ZERO);
        let endpoint = "127.0.0.1:12345".parse().unwrap();
        routes.add_candidate(7, RouteKind::P2PDirect, endpoint);
        assert!(routes.observe(
            7,
            endpoint,
            RouteMetrics {
                rtt_ms: 10.0,
                packets_sent: 10,
                packets_received: 10,
                ..Default::default()
            },
            Instant::now() - Duration::from_secs(10)
        ));
        assert!(routes.best(7, true).is_none());
    }

    #[test]
    fn recent_loss_free_window_allows_requalification() {
        let mut routes = RouteManager::new(Duration::from_secs(20));
        let endpoint = "127.0.0.1:12345".parse().unwrap();
        routes.add_candidate(7, RouteKind::P2PDirect, endpoint);
        let start = Instant::now();
        for n in 1..=300 {
            routes.observe(
                7,
                endpoint,
                RouteMetrics {
                    rtt_ms: 10.0,
                    packets_sent: n,
                    packets_received: n,
                    ..Default::default()
                },
                start + Duration::from_millis(n * 75),
            );
        }
        assert!(routes.best(7, true).is_some());
        routes.degrade_route(7, endpoint);
        for n in 1..=800 {
            routes.observe(
                7,
                endpoint,
                RouteMetrics {
                    rtt_ms: 10.0,
                    packets_sent: 400 + n,
                    packets_received: 300 + n,
                    ..Default::default()
                },
                start + Duration::from_millis(30000 + n * 75),
            );
        }
        assert!(routes.best(7, true).is_some());
        assert_eq!(
            routes.best(7, true).unwrap().metrics.packet_loss_percent,
            0.0
        );
    }
}
