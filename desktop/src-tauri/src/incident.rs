//! Bounded, in-memory diagnostic history. Never samples audio or message content.
use crate::CoreSnapshot;
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    at_ms: u64,
    connection: String,
    diagnostics: crate::DiagnosticSnapshot,
    muted: bool,
    deafened: bool,
    participants: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    schema: &'static str,
    app_version: &'static str,
    platform: &'static str,
    marked_at_ms: u64,
    finished_at_ms: Option<u64>,
    reason: String,
    completion: String,
    samples: Vec<Sample>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub collecting: bool,
    pub remaining_seconds: u64,
    pub report: Option<Report>,
}
#[derive(Default)]
pub struct Recorder {
    history: VecDeque<Sample>,
    scope: String,
    last_sample: Option<u64>,
    pending_until: Option<u64>,
    report: Option<Report>,
}
impl Recorder {
    pub fn sample(&mut self, snapshot: &CoreSnapshot, now: u64) {
        let channel = snapshot
            .users
            .iter()
            .find(|u| u.session_id == snapshot.self_session_id)
            .map(|u| u.channel_id.as_str())
            .unwrap_or("0");
        let joined = snapshot.connection == "connected" && channel != "0";
        let scope = if joined {
            format!(
                "{}|{}|{}",
                snapshot.server_address, snapshot.self_session_id, channel
            )
        } else {
            String::new()
        };
        if scope != self.scope {
            self.finish(now, "connection_changed");
            self.history.clear();
            self.last_sample = None;
            self.scope = scope;
        }
        if !joined {
            return;
        }
        if self
            .last_sample
            .is_some_and(|last| now.saturating_sub(last) < 1000)
        {
            return;
        }
        self.last_sample = Some(now);
        let sample = Sample {
            at_ms: now,
            connection: snapshot.connection.clone(),
            diagnostics: snapshot.diagnostics.clone(),
            muted: snapshot.muted,
            deafened: snapshot.deafened,
            participants: snapshot
                .users
                .iter()
                .filter(|u| u.channel_id == channel)
                .count(),
        };
        self.history.push_back(sample.clone());
        while self.history.len() > 61
            || self
                .history
                .front()
                .is_some_and(|s| now.saturating_sub(s.at_ms) > 60_000)
        {
            self.history.pop_front();
        }
        if self.pending_until.is_some() {
            if let Some(report) = &mut self.report {
                if report.samples.len() < 72 {
                    report.samples.push(sample);
                }
            }
        }
        if self.pending_until.is_some_and(|end| now >= end) {
            self.finish(now, "complete");
        }
    }
    fn finish(&mut self, now: u64, completion: &str) {
        if self.pending_until.take().is_some() {
            if let Some(report) = &mut self.report {
                report.finished_at_ms = Some(now);
                report.completion = completion.into();
            }
        }
    }
    pub fn mark(&mut self, reason: &str, now: u64) -> Result<(), String> {
        if !matches!(reason, "interruption" | "robotic" | "missing" | "other") {
            return Err("请选择问题类型".into());
        }
        if self.pending_until.is_some() {
            return Err("正在补充问题后的指标，请稍候".into());
        }
        if self.scope.is_empty() || self.history.is_empty() {
            return Err("加入语音房间后才能记录问题".into());
        }
        self.report = Some(Report {
            schema: "edenvoice.voice-diagnostics.v1",
            app_version: env!("CARGO_PKG_VERSION"),
            platform: std::env::consts::OS,
            marked_at_ms: now,
            finished_at_ms: None,
            reason: reason.into(),
            completion: "collecting".into(),
            samples: self.history.iter().cloned().collect(),
        });
        self.pending_until = Some(now.saturating_add(10_000));
        Ok(())
    }
    pub fn status(&self, now: u64) -> Status {
        Status {
            available: !self.history.is_empty() && !self.scope.is_empty(),
            collecting: self.pending_until.is_some(),
            remaining_seconds: self
                .pending_until
                .map(|end| end.saturating_sub(now).div_ceil(1000))
                .unwrap_or(0),
            report: self.report.clone(),
        }
    }
    pub fn clear(&mut self) {
        self.report = None;
        self.pending_until = None;
    }
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> CoreSnapshot {
        let mut s = CoreSnapshot::default();
        s.connection = "connected".into();
        s.server_address = "private-server".into();
        s.self_session_id = "1".into();
        s.users = vec![crate::UserSnapshot {
            game_activity: None,
            session_id: "1".into(),
            client_id: "private-id".into(),
            nickname: "Private name".into(),
            avatar: String::new(),
            channel_id: "2".into(),
            muted: false,
            deafened: false,
            admin_muted: false,
            locally_muted: false,
            speaking: false,
            server_role: String::new(),
            channel_role: String::new(),
            volume: 1.0,
        }];
        s
    }
    #[test]
    fn captures_bounded_before_after_without_private_content() {
        let s = snapshot();
        let mut r = Recorder::default();
        for t in 0..120 {
            r.sample(&s, t * 1000);
        }
        assert_eq!(r.history.len(), 61);
        r.mark("robotic", 119000).unwrap();
        assert!(r.mark("other", 119000).is_err());
        for t in 120..130 {
            r.sample(&s, t * 1000);
        }
        let status = r.status(129000);
        assert!(!status.collecting);
        let report = status.report.unwrap();
        assert_eq!(report.samples.len(), 71);
        assert_eq!(report.samples[0].at_ms, 59000);
        let json = serde_json::to_string(&report).unwrap();
        for secret in ["private-server", "private-id", "Private name"] {
            assert!(!json.contains(secret));
        }
        assert!(!crate::contains_sensitive_diagnostic_key(
            &serde_json::to_value(&report).unwrap()
        ));
    }
    #[test]
    fn switching_room_finishes_and_separates_history() {
        let mut s = snapshot();
        let mut r = Recorder::default();
        r.sample(&s, 1000);
        r.mark("interruption", 1000).unwrap();
        s.users[0].channel_id = "3".into();
        r.sample(&s, 2000);
        assert_eq!(
            r.status(2000).report.unwrap().completion,
            "connection_changed"
        );
        assert_eq!(r.history.len(), 1);
        r.clear();
        assert!(r.status(2000).report.is_none());
        s.connection = "disconnected".into();
        r.sample(&s, 3000);
        assert!(r.mark("missing", 3000).is_err());
    }
}
