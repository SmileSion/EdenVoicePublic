use edenvoice_client::protocol::screenshare::{Request, Response, Settings};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct ScreenRequest {
    pub request_id: String,
    pub action: String,
    pub channel_id: String,
    #[serde(default)]
    pub share_id: String,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub sdp: String,
    #[serde(default)]
    pub paused: Option<bool>,
}
impl ScreenRequest {
    pub fn into_core(self) -> Result<Request, String> {
        if self.request_id.len() > 64 || self.share_id.len() > 64 || self.sdp.len() > 48 * 1024 {
            return Err("屏幕共享请求过大".into());
        }
        if !matches!(self.action.as_str(), "status" | "publish" | "subscribe" | "leave" | "stop" | "cancel" | "pause") {
            return Err("无效的屏幕共享操作".into());
        }
        if self.action == "pause" && self.paused.is_none() {
            return Err("请明确屏幕共享暂停状态".into());
        }
        Ok(Request {
            request_id: self.request_id,
            action: self.action,
            channel_id: self.channel_id.parse().map_err(|_| "无效的房间 ID")?,
            share_id: self.share_id,
            settings: self.settings,
            sdp: self.sdp,
            paused: self.paused,
        })
    }
}
// JavaScript must not round random 64-bit session identifiers.
pub fn event(message: Response, server: &str, session: Option<u64>) -> serde_json::Value {
    let revision = message.revision.to_string();
    let channel = message.channel_id.to_string();
    let host = message.host_id.to_string();
    let mut value = serde_json::to_value(message).unwrap_or_default();
    value["revision"] = revision.into();
    value["channel_id"] = channel.into();
    value["host_id"] = host.into();
    value["server_address"] = server.into();
    value["self_session_id"] = session.map(|v| v.to_string()).unwrap_or_default().into();
    value
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn privacy_pause_requires_explicit_state_and_reaches_frontend() {
        let missing: ScreenRequest = serde_json::from_str(r#"{"request_id":"p","action":"pause","channel_id":"7"}"#).unwrap();
        assert!(missing.into_core().is_err());
        for paused in [true, false] {
            let value = serde_json::json!({"request_id":"p","action":"pause","channel_id":"7","paused":paused});
            let request: ScreenRequest = serde_json::from_value(value).unwrap();
            assert_eq!(request.into_core().unwrap().paused, Some(paused));
            let response = Response { paused: Some(paused), ..Default::default() };
            assert_eq!(event(response, "test", Some(1))["paused"], paused);
        }
    }
    #[test]
    fn identifiers_are_exact_in_both_directions() {
        let request: ScreenRequest = serde_json::from_str(r#"{"request_id":"a","action":"status","channel_id":"18446744073709551615"}"#).unwrap();
        assert_eq!(request.into_core().unwrap().channel_id, u64::MAX);
        let response = Response { revision:u64::MAX, channel_id:u64::MAX, host_id:u64::MAX, ..Default::default() };
        let value = event(response,"test",Some(u64::MAX));
        assert_eq!(value["host_id"],u64::MAX.to_string());
        assert_eq!(value["revision"],u64::MAX.to_string());
        assert_eq!(value["self_session_id"],u64::MAX.to_string());
    }
}
