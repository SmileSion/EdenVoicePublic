//! Small room events and bounded image chunks travel only on the control lane.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

pub const MAX_IMAGE_BYTES: usize = 256 * 1024;
pub const MAX_CHUNK_BYTES: usize = 8192;
pub const MAX_CHUNK_BASE64: usize = MAX_CHUNK_BYTES.div_ceil(3) * 4;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub reference: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub request_id: String,
    pub channel_id: u64,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        if self.request_id.is_empty() || self.request_id.len() > 64 || self.channel_id == 0 {
            return Err("无效的房间互动请求".into());
        }
        if self.upload_id.as_ref().is_some_and(|v| v.len() > 128)
            || self.reference.as_ref().is_some_and(|v| v.len() > 128)
            || self.emoji.as_ref().is_some_and(|v| v.len() > 32)
            || self
                .data
                .as_ref()
                .is_some_and(|v| v.len() > MAX_CHUNK_BASE64)
            || self.offset.is_some_and(|v| v as usize > MAX_IMAGE_BYTES)
        {
            return Err("房间互动请求过大".into());
        }
        match self.action.as_str() {
            "status" => {}
            "away" if self.active.is_some() => {}
            "reaction"
                if self
                    .emoji
                    .as_deref()
                    .is_some_and(|v| matches!(v, "👍" | "❤️" | "😂" | "🎉" | "👏" | "☕")) =>
                {}
            "image_begin"
                if self
                    .bytes
                    .is_some_and(|v| v > 0 && v as usize <= MAX_IMAGE_BYTES)
                    && self.width.is_some_and(|v| v > 0 && v <= 1600)
                    && self.height.is_some_and(|v| v > 0 && v <= 1600) => {}
            "image_chunk" if self.upload_id.as_ref().is_some_and(|v| !v.is_empty()) => {
                validate_chunk(self.data.as_deref().unwrap_or_default())?;
            }
            "image_commit" | "image_cancel"
                if self.upload_id.as_ref().is_some_and(|v| !v.is_empty()) => {}
            "image_get" if self.reference.as_ref().is_some_and(|v| !v.is_empty()) => {}
            _ => return Err("无效的房间互动操作".into()),
        }
        Ok(())
    }
}

fn validate_chunk(data: &str) -> Result<(), String> {
    if data.len() > MAX_CHUNK_BASE64 {
        return Err("图片分块过大".into());
    }
    let decoded = STANDARD.decode(data).map_err(|_| "无效的图片分块")?;
    if decoded.is_empty() || decoded.len() > MAX_CHUNK_BYTES {
        return Err("图片分块过大或为空".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Away {
    pub session_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    pub request_id: String,
    pub channel_id: u64,
    pub kind: String,
    pub code: String,
    pub message: String,
    pub retry_after_ms: u64,
    pub away: Vec<Away>,
    pub session_id: u64,
    pub active: bool,
    pub emoji: String,
    pub expires_at_unix_ms: i64,
    pub upload_id: String,
    pub offset: u32,
    pub reference: String,
    pub data: String,
    pub eof: bool,
    pub image: Option<Image>,
}

impl Event {
    pub fn is_bounded(&self) -> bool {
        self.request_id.len() <= 64
            && self.away.len() <= 1024
            && self.reference.len() <= 128
            && self.upload_id.len() <= 128
            && self.emoji.len() <= 32
            && self.message.len() <= 2048
            && (self.data.is_empty() || validate_chunk(&self.data).is_ok())
            && self.image.as_ref().is_none_or(|image| {
                image.mime == "image/jpeg"
                    && image.reference.len() <= 128
                    && image.bytes as usize <= MAX_IMAGE_BYTES
                    && image.width <= 1600
                    && image.height <= 1600
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_are_on_decoded_bytes_and_reject_invalid_base64() {
        assert!(validate_chunk(&STANDARD.encode(vec![0; MAX_CHUNK_BYTES])).is_ok());
        assert!(validate_chunk(&STANDARD.encode(vec![0; MAX_CHUNK_BYTES + 1])).is_err());
        assert!(validate_chunk("@@@@").is_err());
    }
    #[test]
    fn explicit_away_boolean_is_required_and_false_is_serialized() {
        let mut request = Request {
            request_id: "one".into(),
            channel_id: 1,
            action: "away".into(),
            active: Some(false),
            ..Default::default()
        };
        assert!(request.validate().is_ok());
        assert_eq!(serde_json::to_value(&request).unwrap()["active"], false);
        request.active = None;
        assert!(request.validate().is_err());
    }
    #[test]
    fn old_chat_records_stay_compatible_and_new_records_contain_metadata_only() {
        let legacy = serde_json::json!({"id":1,"channel_id":1,"sender_session_id":7,"sender_name":"A","text":"hello","sent_at_unix_ms":2});
        let mut message: crate::protocol::control::TextMessage =
            serde_json::from_value(legacy).unwrap();
        assert!(message.image.is_none());
        message.image = Some(Image {
            reference: "room-image:abc".into(),
            mime: "image/jpeg".into(),
            width: 100,
            height: 80,
            bytes: 1000,
        });
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value["image"]["reference"], "room-image:abc");
        assert!(value["image"].get("data").is_none());
    }
}
