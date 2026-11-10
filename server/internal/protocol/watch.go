package protocol

import "edenvoice/server/internal/watchparty"

const (
	TypeWatchRequest MessageType = "watch_request"
	TypeWatchState   MessageType = "watch_state"
)

// ChannelID is an expected room, never authority. The server verifies the
// authenticated connection is still in that voice room for every request.
type WatchRequest struct {
	ChannelID       uint64           `json:"channel_id"`
	Epoch           uint64           `json:"epoch"`
	RequestID       uint64           `json:"request_id"`
	Action          string           `json:"action"`
	Position        float64          `json:"position"`
	File            *watchparty.File `json:"file,omitempty"`
	Ready           bool             `json:"ready"`
	TargetSessionID uint64           `json:"target_session_id,omitempty"`
}

// Calculated for each authenticated recipient, never accepted from requests.
type WatchPermissions struct {
	Role            string `json:"role"`
	CanControl      bool   `json:"can_control"`
	CanClose        bool   `json:"can_close"`
	CanTransferHost bool   `json:"can_transfer_host"`
}

type WatchState struct {
	watchparty.Snapshot
	RequestID   uint64            `json:"request_id"`
	ActorID     uint64            `json:"actor_id"`
	Action      string            `json:"action"`
	Error       string            `json:"error,omitempty"`
	Permissions *WatchPermissions `json:"permissions,omitempty"`
}
