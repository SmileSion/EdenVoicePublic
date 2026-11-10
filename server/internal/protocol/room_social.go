package protocol

const (
	TypeRoomSocialRequest  MessageType = "room_social_request"
	TypeRoomSocialEvent    MessageType = "room_social_event"
	MaxRoomImageBytes                  = 256 * 1024
	MaxRoomImageChunkBytes             = 8 * 1024
	MaxRoomImageSide                   = 1600
	MaxRoomImagePixels                 = 1600 * 1600
)

type ChatImage struct {
	Reference string `json:"reference"`
	MIME      string `json:"mime"`
	Width     int    `json:"width"`
	Height    int    `json:"height"`
	Bytes     int    `json:"bytes"`
}

type RoomSocialRequest struct {
	RequestID string `json:"request_id"`
	ChannelID uint64 `json:"channel_id"`
	Action    string `json:"action"`
	Active    *bool  `json:"active,omitempty"`
	Emoji     string `json:"emoji,omitempty"`
	UploadID  string `json:"upload_id,omitempty"`
	Reference string `json:"reference,omitempty"`
	Bytes     int    `json:"bytes,omitempty"`
	Width     int    `json:"width,omitempty"`
	Height    int    `json:"height,omitempty"`
	Offset    int    `json:"offset,omitempty"`
	Data      string `json:"data,omitempty"`
}

type AwayPresence struct {
	SessionID uint64 `json:"session_id"`
}

type RoomSocialEvent struct {
	RequestID       string         `json:"request_id,omitempty"`
	ChannelID       uint64         `json:"channel_id"`
	Kind            string         `json:"kind"`
	Code            string         `json:"code,omitempty"`
	Message         string         `json:"message,omitempty"`
	RetryAfterMS    int64          `json:"retry_after_ms,omitempty"`
	Away            []AwayPresence `json:"away,omitempty"`
	SessionID       uint64         `json:"session_id,omitempty"`
	Active          bool           `json:"active"`
	Emoji           string         `json:"emoji,omitempty"`
	ExpiresAtUnixMS int64          `json:"expires_at_unix_ms,omitempty"`
	UploadID        string         `json:"upload_id,omitempty"`
	Reference       string         `json:"reference,omitempty"`
	Offset          int            `json:"offset"`
	Data            string         `json:"data,omitempty"`
	EOF             bool           `json:"eof,omitempty"`
	Image           *ChatImage     `json:"image,omitempty"`
}

func ValidRoomImageDimensions(width, height, size int) bool {
	return width > 0 && height > 0 && width <= MaxRoomImageSide && height <= MaxRoomImageSide && width*height <= MaxRoomImagePixels && size > 0 && size <= MaxRoomImageBytes
}
func ValidRoomReaction(emoji string) bool {
	switch emoji {
	case "👍", "❤️", "😂", "🎉", "👏", "☕":
		return true
	}
	return false
}
