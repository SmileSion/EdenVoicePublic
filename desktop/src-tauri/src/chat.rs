use crate::ChatMessageSnapshot;
use edenvoice_client::{application::Application, protocol::control::ChatQuery};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::mpsc::SyncSender,
    time::{Duration, Instant},
};

pub type Reply = SyncSender<Result<Page, String>>;
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub before: String,
    pub server: String,
    pub session: String,
    pub channel: String,
    pub cursor: String,
    pub query: String,
}
impl Request {
    fn matches(&self, core: &Application) -> bool {
        core.state().announcement_scope == self.fingerprint
            && core.server_address() == self.server
            && core
                .state()
                .self_session
                .map(|id| id.to_string())
                .as_deref()
                == Some(self.session.as_str())
            && core
                .state()
                .self_session
                .and_then(|id| core.state().users.get(&id))
                .is_some_and(|user| user.channel_id.to_string() == self.channel)
            && core.state().connection == edenvoice_client::model::ConnectionState::Connected
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    messages: Vec<ChatMessageSnapshot>,
    next_cursor: String,
}
#[derive(Default)]
pub struct Coordinator {
    next: u64,
    pending: HashMap<String, (Request, Instant, Reply)>,
}
impl Coordinator {
    pub fn request(&mut self, core: &Application, request: Request, reply: Reply) {
        if !request.matches(core)
            || self.pending.len() >= 4
            || request.query.len() > 256
            || request.cursor.len() > 20
            || request.before.len() > 20
            || (!request.before.is_empty() && request.before.parse::<u64>().is_err())
        {
            let _ = reply.try_send(Err("房间已改变或查询繁忙，请重试".into()));
            return;
        }
        self.next = self.next.wrapping_add(1);
        let id = self.next.to_string();
        let channel = match request.channel.parse() {
            Ok(channel) => channel,
            Err(_) => {
                let _ = reply.try_send(Err("无效房间".into()));
                return;
            }
        };
        let wire = ChatQuery {
            before_id: request.before.clone(),
            request_id: id.clone(),
            channel_id: channel,
            cursor: request.cursor.clone(),
            query: request.query.clone(),
        };
        match core.query_chat(wire) {
            Ok(()) => {
                self.pending.insert(id, (request, Instant::now(), reply));
            }
            Err(error) => {
                let _ = reply.try_send(Err(error));
            }
        }
    }
    pub fn tick(&mut self, core: &mut Application) {
        self.pending.retain(|_, (context, at, reply)| {
            if !context.matches(core) || at.elapsed() > Duration::from_secs(10) {
                let _ = reply.try_send(Err("查询超时或房间已改变，请重试".into()));
                false
            } else {
                true
            }
        });
        for result in core.take_chat_query_results() {
            if let Some((context, _, reply)) = self.pending.remove(&result.request_id) {
                if context.channel != result.channel_id.to_string() {
                    let _ = reply.try_send(Err("查询房间不匹配".into()));
                    continue;
                }
                if !result.error.is_empty() {
                    let _ = reply.try_send(Err(result.error));
                    continue;
                }
                let messages = result
                    .messages
                    .into_iter()
                    .map(|m| ChatMessageSnapshot {
                        id: m.id.to_string(),
                        channel_id: m.channel_id.to_string(),
                        sender_session_id: m.sender_session_id.to_string(),
                        sender_client_id: m.sender_client_id,
                        sender_name: m.sender_name,
                        sender_avatar: m.sender_avatar,
                        text: m.text,
                        sent_at_unix_ms: m.sent_at_unix_ms,
                        image: m.image,
                    })
                    .collect();
                let _ = reply.try_send(Ok(Page {
                    messages,
                    next_cursor: result.next_cursor,
                }));
            }
        }
    }
}
