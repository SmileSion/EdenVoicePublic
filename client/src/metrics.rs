#[derive(Clone, Debug, Default)]
pub struct NetworkStats {
    pub rtt_ms: Option<u64>,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
}
