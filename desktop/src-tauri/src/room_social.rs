use edenvoice_client::{
    application::Application,
    model::ConnectionState,
    protocol::room_social::{Event, Request},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

pub type Reply = mpsc::SyncSender<Result<serde_json::Value, String>>;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub server_address: String,
    pub self_session_id: String,
    pub channel_id: String,
    #[serde(default)]
    pub server_fingerprint: String,
}

impl Context {
    fn matches(&self, core: &Application) -> bool {
        self.matches_state(core.server_address(), core.state())
    }

    fn matches_state(&self, server: &str, state: &edenvoice_client::model::AppState) -> bool {
        state.connection == ConnectionState::Connected
            && server == self.server_address
            && state.self_session.map(|id| id.to_string()).as_deref()
                == Some(self.self_session_id.as_str())
            && state
                .self_session
                .and_then(|id| state.users.get(&id))
                .map(|user| user.channel_id.to_string())
                .as_deref()
                == Some(self.channel_id.as_str())
            && (self.server_fingerprint.is_empty()
                || self.server_fingerprint == state.announcement_scope)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UiRequest {
    pub request_id: String,
    pub channel_id: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl UiRequest {
    fn into_core(self) -> Result<Request, String> {
        let channel_id: u64 = self.channel_id.parse().map_err(|_| "无效的房间 ID")?;
        let mut fields = self.fields;
        fields.insert("request_id".into(), self.request_id.into());
        fields.insert("channel_id".into(), channel_id.into());
        let request: Request =
            serde_json::from_value(fields.into()).map_err(|_| "无效的房间互动请求")?;
        request.validate()?;
        Ok(request)
    }
}

struct Pending {
    context: Context,
    epoch: u64,
    at: Instant,
    reply: Reply,
}

#[derive(Default)]
pub struct Coordinator {
    pending: HashMap<String, Pending>,
}

impl Coordinator {
    pub fn request(
        &mut self,
        core: &mut Application,
        context: Context,
        request: UiRequest,
        reply: Reply,
    ) {
        let result = (|| {
            if !context.matches(core) || request.channel_id != context.channel_id {
                return Err("连接或房间已改变，请重试".into());
            }
            if self.pending.len() >= 16 {
                return Err("房间互动请求繁忙，请稍后重试".into());
            }
            let request = request.into_core()?;
            if self.pending.contains_key(&request.request_id) {
                return Err("房间互动请求 ID 重复".into());
            }
            let id = request.request_id.clone();
            core.request_room_social(request)?;
            self.pending.insert(
                id,
                Pending {
                    context,
                    epoch: core.state().room_social_epoch,
                    at: Instant::now(),
                    reply: reply.clone(),
                },
            );
            Ok(())
        })();
        if let Err(error) = result {
            let _ = reply.try_send(Err(error));
        }
    }

    pub fn tick(&mut self, core: &mut Application) -> Vec<serde_json::Value> {
        self.pending.retain(|_, pending| {
            let error = if !pending.context.matches(core)
                || pending.epoch != core.state().room_social_epoch
            {
                Some("连接或房间已改变，请重试")
            } else if pending.at.elapsed() >= Duration::from_secs(15) {
                Some("房间互动请求超时，请重试")
            } else {
                None
            };
            if let Some(error) = error {
                let _ = pending.reply.try_send(Err(error.into()));
                return false;
            }
            true
        });
        let channel_id = core
            .state()
            .self_session
            .and_then(|id| core.state().users.get(&id))
            .map(|user| user.channel_id);
        let mut events = Vec::new();
        for message in core.take_room_social_events() {
            if channel_id != Some(message.channel_id) {
                continue;
            }
            let request_id = message.request_id.clone();
            let value = event(
                message,
                core.server_address(),
                core.state().self_session,
                &core.state().announcement_scope,
            );
            if let Some(pending) = self.pending.remove(&request_id) {
                let _ = pending.reply.try_send(Ok(value.clone()));
            }
            // Image bodies are request responses only. Never multicast them to
            // overlay windows or retain them in the frequent core snapshots.
            if value["kind"] != "image_data" {
                events.push(value);
            }
        }
        events
    }
}

fn event(
    message: Event,
    server: &str,
    session: Option<u64>,
    fingerprint: &str,
) -> serde_json::Value {
    let channel_id = message.channel_id.to_string();
    let session_id = message.session_id.to_string();
    let away: Vec<_> = message
        .away
        .iter()
        .map(|away| serde_json::json!({"session_id":away.session_id.to_string()}))
        .collect();
    let mut value = serde_json::to_value(message).unwrap_or_default();
    value["channel_id"] = channel_id.into();
    value["session_id"] = session_id.into();
    value["away"] = away.into();
    value["server_address"] = server.into();
    value["self_session_id"] = session.map(|id| id.to_string()).unwrap_or_default().into();
    value["server_fingerprint"] = fingerprint.into();
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_rejects_different_servers_identities_sessions_and_rooms() {
        let context = Context {
            server_address: "example:7001".into(),
            self_session_id: "7".into(),
            channel_id: "3".into(),
            server_fingerprint: "identity-a".into(),
        };
        let mut state = edenvoice_client::model::AppState {
            connection: ConnectionState::Connected,
            self_session: Some(7),
            announcement_scope: "identity-a".into(),
            ..Default::default()
        };
        state.users.insert(
            7,
            edenvoice_client::protocol::control::User {
                session_id: 7,
                channel_id: 3,
                ..Default::default()
            },
        );
        assert!(context.matches_state("example:7001", &state));
        assert!(!context.matches_state("other:7001", &state));
        state.announcement_scope = "identity-b".into();
        assert!(!context.matches_state("example:7001", &state));
        state.announcement_scope = "identity-a".into();
        state.move_user(7, 4);
        assert!(!context.matches_state("example:7001", &state));
        state.move_user(7, 3);
        state.self_session = Some(8);
        assert!(!context.matches_state("example:7001", &state));
    }
    #[test]
    fn identifiers_survive_the_javascript_boundary_exactly() {
        let request: UiRequest = serde_json::from_value(serde_json::json!({"request_id":"a","action":"status","channel_id":u64::MAX.to_string()})).unwrap();
        assert_eq!(request.into_core().unwrap().channel_id, u64::MAX);
        let value = event(
            Event {
                channel_id: u64::MAX,
                session_id: u64::MAX,
                away: vec![edenvoice_client::protocol::room_social::Away {
                    session_id: u64::MAX,
                }],
                ..Default::default()
            },
            "a",
            Some(u64::MAX),
            "identity",
        );
        assert_eq!(value["session_id"], u64::MAX.to_string());
        assert_eq!(value["away"][0]["session_id"], u64::MAX.to_string());
        assert_eq!(value["server_fingerprint"], "identity");
    }
}
