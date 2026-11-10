package serverstate

import (
	"edenvoice/server/internal/protocol"
	"errors"
	"testing"
	"time"
)

type socialRecorder struct{ messages []protocol.Envelope }

func (r *socialRecorder) Send(e protocol.Envelope) error {
	r.messages = append(r.messages, e)
	return nil
}
func awayValue(active bool) *bool { return &active }
func TestAwayMutesIndefinitelyUnmuteClearsAndLegacyNotified(t *testing.T) {
	s := newTestState()
	sender, legacySender, otherSender := &socialRecorder{}, &socialRecorder{}, &socialRecorder{}
	a := s.AddAuthenticatedSession("A", "a", "", "", sender, nil, nil)
	legacy := s.AddAuthenticatedSession("old", "old", "", "", legacySender, nil, nil)
	other := s.AddAuthenticatedSession("other", "other", "", "", otherSender, nil, nil)
	_ = s.Join(a.ID, 1)
	_ = s.Join(legacy.ID, 1)
	_ = s.Join(other.ID, 2)
	s.EnableRoomSocial(a.ID)
	s.EnableRoomSocial(other.ID)
	now := time.Now()
	setAway := func(active bool) {
		t.Helper()
		if err := s.ApplyRoomSocial(a.ID, protocol.RoomSocialRequest{ChannelID: 1, Action: "away", Active: awayValue(active)}, now); err != nil {
			t.Fatal(err)
		}
	}
	setAway(true)
	user, _ := s.User(a.ID)
	if !user.Muted || user.Deafened || s.CanSpeak(a.ID) {
		t.Fatal("away did not atomically mute microphone")
	}
	if len(sender.messages) != 2 || sender.messages[0].Type != protocol.TypeUserStateChanged || sender.messages[1].Type != protocol.TypeRoomSocialEvent {
		t.Fatal("mute must be sent before away acknowledgment")
	}
	for _, recipient := range []*socialRecorder{legacySender, otherSender} {
		if len(recipient.messages) != 1 || recipient.messages[0].Type != protocol.TypeUserStateChanged {
			t.Fatal("legacy mute notification missing or social state leaked")
		}
		var voice protocol.UserStateChanged
		_ = recipient.messages[0].Decode(&voice)
		if !voice.Muted || voice.SessionID != a.ID {
			t.Fatal("wrong mute event")
		}
	}
	if err := s.ApplyRoomSocial(a.ID, protocol.RoomSocialRequest{ChannelID: 1, Action: "away"}, now); err == nil {
		t.Fatal("missing active boolean accepted")
	}
	// Status has no clock or expiry; unrelated future events do not clear away.
	_ = s.ApplyRoomSocial(a.ID, protocol.RoomSocialRequest{ChannelID: 1, Action: "reaction", Emoji: "👍"}, now.Add(48*time.Hour))
	away, _ := s.RoomSocialStatus(a.ID, 1)
	if len(away) != 1 || away[0].SessionID != a.ID {
		t.Fatal("away did not persist")
	}
	_ = s.SetDeafened(a.ID, true)
	_ = s.SetDeafened(a.ID, false)
	_ = s.SetMuted(a.ID, true)
	away, _ = s.RoomSocialStatus(a.ID, 1)
	if len(away) != 1 {
		t.Fatal("deafen-only or mute change cleared away")
	}
	setAway(false)
	user, _ = s.User(a.ID)
	if !user.Muted {
		t.Fatal("manual presence return opened microphone")
	}
	setAway(true)
	before := len(sender.messages)
	if err := s.SetMuted(a.ID, false); err != nil {
		t.Fatal(err)
	}
	away, _ = s.RoomSocialStatus(a.ID, 1)
	user, _ = s.User(a.ID)
	if len(away) != 0 || user.Muted || !s.CanSpeak(a.ID) {
		t.Fatal("unmute did not clear away")
	}
	if len(sender.messages) != before+1 {
		t.Fatal("unmute did not broadcast away clear")
	}
	var clear protocol.RoomSocialEvent
	_ = sender.messages[len(sender.messages)-1].Decode(&clear)
	if clear.Kind != "away" || clear.Active {
		t.Fatal("wrong unmute clear event")
	}
	setAway(true)
	gen, _ := s.RoomSocialMember(a.ID, 1)
	_ = s.Join(a.ID, 2)
	_ = s.Join(a.ID, 1)
	next, _ := s.RoomSocialMember(a.ID, 1)
	away, _ = s.RoomSocialStatus(a.ID, 1)
	if next == gen || len(away) != 0 {
		t.Fatal("move did not invalidate away")
	}
	user, _ = s.User(a.ID)
	if !user.Muted {
		t.Fatal("room move opened microphone")
	}
	setAway(true)
	s.RemoveSession(a.ID)
	if _, err := s.RoomSocialStatus(a.ID, 1); !errors.Is(err, ErrRoomSocialAccess) {
		t.Fatal("removed session authorized")
	}
}
func TestReactionsIsolatedEphemeralLimitedAndLegacySafe(t *testing.T) {
	s := newTestState()
	aOut, bOut, oldOut := &socialRecorder{}, &socialRecorder{}, &socialRecorder{}
	a := s.AddAuthenticatedSession("A", "a", "", "", aOut, nil, nil)
	b := s.AddAuthenticatedSession("B", "b", "", "", bOut, nil, nil)
	old := s.AddAuthenticatedSession("Old", "old", "", "", oldOut, nil, nil)
	_ = s.Join(a.ID, 1)
	_ = s.Join(b.ID, 2)
	_ = s.Join(old.ID, 1)
	s.EnableRoomSocial(a.ID)
	s.EnableRoomSocial(b.ID)
	now := time.Now()
	req := protocol.RoomSocialRequest{ChannelID: 1, Action: "reaction", Emoji: "👍"}
	if err := s.ApplyRoomSocial(a.ID, req, now); err != nil {
		t.Fatal(err)
	}
	if len(aOut.messages) != 1 || len(bOut.messages) != 0 || len(oldOut.messages) != 0 {
		t.Fatal("reaction leaked across room or to legacy client")
	}
	var event protocol.RoomSocialEvent
	_ = aOut.messages[0].Decode(&event)
	if event.ExpiresAtUnixMS != now.Add(2500*time.Millisecond).UnixMilli() {
		t.Fatal("reaction duration")
	}
	if err := s.ApplyRoomSocial(a.ID, req, now.Add(time.Second)); !errors.Is(err, ErrReactionCooldown) {
		t.Fatal("cooldown bypassed")
	}
	if err := s.ApplyRoomSocial(a.ID, req, now.Add(2*time.Second)); err != nil {
		t.Fatal(err)
	}
	req.ChannelID = 2
	if err := s.ApplyRoomSocial(a.ID, req, now.Add(3*time.Second)); !errors.Is(err, ErrRoomSocialAccess) {
		t.Fatal("cross room reaction accepted")
	}
	req.ChannelID = 1
	req.Emoji = "<script>"
	if err := s.ApplyRoomSocial(a.ID, req, now.Add(4*time.Second)); err == nil {
		t.Fatal("unlisted emoji accepted")
	}
	if len(s.ChatHistory(1)) != 0 {
		t.Fatal("ephemeral reaction persisted")
	}
	if away, _ := s.RoomSocialStatus(a.ID, 1); len(away) != 0 {
		t.Fatal("reaction in status")
	}
	anonymous := s.AddSession("anon", nil)
	_ = s.Join(anonymous.ID, 1)
	if _, ok := s.RoomSocialMember(anonymous.ID, 1); ok {
		t.Fatal("unauthenticated client accepted")
	}
}
