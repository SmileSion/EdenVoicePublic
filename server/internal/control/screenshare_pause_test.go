package control

import (
	"testing"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"
)

func TestPrivacyPauseDispatchRequiresExplicitBoolAndCurrentRoom(t *testing.T) {
	s, _ := socialServer(t)
	if err := s.ConfigureScreenSharing(screenshare.DefaultConfig()); err != nil {
		t.Fatal(err)
	}
	defer s.CloseScreenSharing()
	id, out := socialMember(t, s, 1)
	for _, tc := range []struct {
		request screenshare.Request
		want    string
	}{
		{screenshare.Request{RequestID: "missing", ChannelID: 1, ShareID: "none", Action: "pause"}, "隐私暂停需要明确的 paused 布尔值"},
		{screenshare.Request{RequestID: "wrong-room", ChannelID: 2, ShareID: "none", Action: "pause", Paused: socialActive(true)}, "请先进入同一个语音房间"},
		{screenshare.Request{RequestID: "stale", ChannelID: 1, ShareID: "none", Action: "pause", Paused: socialActive(false)}, "共享已结束"},
	} {
		env, _ := protocol.NewEnvelope(protocol.TypeScreenShareRequest, tc.request)
		if err := s.dispatch(id, env, out); err != nil {
			t.Fatal(err)
		}
		response := screenshare.Response{}
		if err := out.messages[len(out.messages)-1].Decode(&response); err != nil {
			t.Fatal(err)
		}
		if response.RequestID != tc.request.RequestID || response.Action != "pause" || response.Error != tc.want || response.Paused {
			t.Fatalf("wrong pause response: %+v", response)
		}
	}
	invalid, _ := protocol.NewEnvelope(protocol.TypeScreenShareRequest, map[string]any{"action": "pause", "channel_id": 1, "paused": "true"})
	if err := s.dispatch(id, invalid, out); err == nil {
		t.Fatal("non-boolean privacy state accepted")
	}
}
