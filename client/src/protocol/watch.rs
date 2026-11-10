use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchFile {
    pub name: String,
    pub size: u64,
    pub duration: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchRequest {
    pub channel_id: u64,
    pub epoch: u64,
    pub request_id: u64,
    pub action: String,
    #[serde(default)]
    pub target_session_id: u64,
    pub position: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<WatchFile>,
    pub ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchMember {
    pub session_id: u64,
    pub file: Option<WatchFile>,
    pub ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchPermissions {
    pub role: String,
    pub can_control: bool,
    pub can_close: bool,
    pub can_transfer_host: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchState {
    pub channel_id: u64,
    pub host_id: u64,
    pub epoch: u64,
    pub revision: u64,
    pub position: f64,
    pub paused: bool,
    // Go emits null on leave/error, not an empty collection.
    pub members: Option<Vec<WatchMember>>,
    pub request_id: u64,
    pub actor_id: u64,
    pub action: String,
    pub error: String,
    pub permissions: Option<WatchPermissions>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::control::{Envelope, MessageType};
    #[test]
    fn go_watch_state_wire_shape_decodes_without_local_paths() {
        let raw = r#"{"type":"watch_state","channel_id":7,"epoch":2,"revision":9,"position":12.5,"paused":false,"members":[{"session_id":3,"file":{"name":"movie.mkv","size":10,"duration":100},"ready":true}],"request_id":4,"actor_id":3,"action":"play"}"#;
        let envelope: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(envelope.kind, MessageType::WatchState);
        let state: WatchState = envelope.decode().unwrap();
        assert_eq!(state.channel_id, 7);
        assert_eq!(
            state.members.unwrap()[0].file.as_ref().unwrap().name,
            "movie.mkv"
        );
        assert!(!raw.contains("path"));
    }
    #[test]
    fn moderation_permissions_and_transfer_decode() {
        let state: WatchState = serde_json::from_str(r#"{"channel_id":7,"permissions":{"role":"channel_admin","can_control":false,"can_close":true,"can_transfer_host":true}}"#).unwrap();
        let permissions = state.permissions.unwrap();
        assert!(permissions.can_close && permissions.can_transfer_host && !permissions.can_control);
        let request = WatchRequest {
            action: "transfer_host".into(),
            target_session_id: 42,
            ..Default::default()
        };
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["target_session_id"], 42);
    }
    #[test]
    fn leave_with_null_members_decodes_as_empty_optional_collection() {
        let raw = r#"{"type":"watch_state","channel_id":7,"epoch":2,"revision":0,"position":0,"paused":false,"members":null,"request_id":4,"actor_id":3,"action":"leave"}"#;
        let envelope: Envelope = serde_json::from_str(raw).unwrap();
        assert!(envelope.decode::<WatchState>().unwrap().members.is_none());
    }
}
