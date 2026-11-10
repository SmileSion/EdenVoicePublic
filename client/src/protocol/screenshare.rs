use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub audio: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub enabled: bool,
    pub max_bitrate_kbps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub allow_audio: bool,
    pub max_viewers: u32,
    pub max_rooms: u32,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub request_id: String,
    pub action: String,
    pub channel_id: u64,
    pub share_id: String,
    pub settings: Settings,
    pub sdp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Response {
    pub revision: u64,
    pub request_id: String,
    pub action: String,
    pub channel_id: u64,
    pub share_id: String,
    pub host_id: u64,
    pub settings: Settings,
    pub policy: Policy,
    pub viewers: u32,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    pub sdp: String,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn privacy_pause_is_explicit_and_old_snapshots_preserve_missing_capability() {
        for paused in [false, true] {
            let request = Request { action: "pause".into(), paused: Some(paused), ..Default::default() };
            let value = serde_json::to_value(&request).unwrap();
            assert_eq!(value["paused"], paused);
            assert_eq!(serde_json::from_value::<Request>(value).unwrap().paused, Some(paused));
        }
        let old: Response = serde_json::from_str(r#"{"action":"status","share_id":"existing"}"#).unwrap();
        assert_eq!(old.paused, None);
        assert!(serde_json::to_value(old).unwrap().get("paused").is_none());
        assert!(serde_json::to_value(Request::default()).unwrap().get("paused").is_none());
    }
}
