package serverstate

import (
	"errors"
	"fmt"
	"net"
	"reflect"
	"testing"
	"time"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
)

func TestHeartbeatExpiryAndTouch(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	state.mu.Lock()
	state.sessions[session.ID].LastHeartbeat = time.Now().Add(-time.Minute)
	state.mu.Unlock()
	if expired := state.Expired(time.Now().Add(-30 * time.Second)); len(expired) != 1 || expired[0] != session.ID {
		t.Fatalf("unexpected expired sessions: %v", expired)
	}
	if !state.Touch(session.ID) {
		t.Fatal("known session was not touched")
	}
	if expired := state.Expired(time.Now().Add(-30 * time.Second)); len(expired) != 0 {
		t.Fatalf("touch did not refresh heartbeat: %v", expired)
	}
}

func TestTouchRejectsUnknownSession(t *testing.T) {
	if newTestState().Touch(999) {
		t.Fatal("unknown session was touched")
	}
}

type testSender struct{ messages []protocol.Envelope }

func (s *testSender) Send(message protocol.Envelope) error {
	s.messages = append(s.messages, message)
	return nil
}

func newTestState() *State { return New(config.Default()) }

func TestSessionLifecycleAndChannelJoin(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", &testSender{})
	if session.ID == 0 {
		t.Fatal("session id was not assigned")
	}
	if err := state.Join(session.ID, 2); err != nil {
		t.Fatal(err)
	}
	user, ok := state.User(session.ID)
	if !ok || user.ChannelID != 2 {
		t.Fatalf("unexpected user: %+v", user)
	}
	if err := state.Join(session.ID, 999); err != ErrUnknownChannel {
		t.Fatalf("expected unknown channel, got %v", err)
	}
	if err := state.Leave(session.ID); err != nil {
		t.Fatal(err)
	}
	user, _ = state.User(session.ID)
	if user.ChannelID != 0 {
		t.Fatalf("leave retained channel %d", user.ChannelID)
	}
	if _, ok := state.RemoveSession(session.ID); !ok {
		t.Fatal("session was not removed")
	}
}

func TestRemoveSessionDestroysMediaSecurity(t *testing.T) {
	state := newTestState()
	inbound, _ := security.NewMediaCipher([32]byte{1}, [6]byte{2}, 1)
	outbound, _ := security.NewMediaCipher([32]byte{3}, [6]byte{4}, 1)
	session := state.AddAuthenticatedSession("Alice", "client", "", "", nil, inbound, outbound)

	removed, ok := state.RemoveSession(session.ID)
	if !ok {
		t.Fatal("session was not removed")
	}
	if removed.MediaInbound != nil || removed.MediaOutbound != nil {
		t.Fatal("removed session exposed destroyed media ciphers")
	}
	if _, _, err := inbound.Seal(nil, 1, 1, nil); err == nil {
		t.Fatal("removed inbound cipher remained usable")
	}
	if _, _, err := outbound.Seal(nil, 1, 1, nil); err == nil {
		t.Fatal("removed outbound cipher remained usable")
	}
}

func TestSessionIDsAreUniqueAndNonZero(t *testing.T) {
	state := newTestState()
	seen := make(map[uint64]bool)
	for range 1000 {
		session := state.AddSession("user", nil)
		if session.ID == 0 || seen[session.ID] {
			t.Fatalf("invalid or duplicate session id %d", session.ID)
		}
		seen[session.ID] = true
	}
}

func TestMuteAndDeafen(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	if err := state.SetDeafened(session.ID, true); err != nil {
		t.Fatal(err)
	}
	user, _ := state.User(session.ID)
	if !user.Deafened || !user.Muted {
		t.Fatalf("deafen must imply mute: %+v", user)
	}
	if err := state.SetDeafened(session.ID, false); err != nil {
		t.Fatal(err)
	}
	user, _ = state.User(session.ID)
	if user.Deafened || !user.Muted {
		t.Fatalf("undeafen must not implicitly unmute: %+v", user)
	}
	if err := state.SetDeafened(session.ID, true); err != nil {
		t.Fatal(err)
	}
	if err := state.SetMuted(session.ID, false); err != nil {
		t.Fatal(err)
	}
	user, _ = state.User(session.ID)
	if user.Deafened || user.Muted {
		t.Fatalf("opening the microphone must also restore speaker audio: %+v", user)
	}
}

func TestServerAndChannelMuteHaveIndependentScope(t *testing.T) {
	state := New(config.Default())
	session := state.AddAuthenticatedSession("user", "client", "USER", "", nil, nil, nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.SetChannelMuted(session.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	if state.CanSpeak(session.ID) {
		t.Fatal("channel mute did not suppress speech in its channel")
	}
	if err := state.Move(session.ID, 2); err != nil {
		t.Fatal(err)
	}
	if !state.CanSpeak(session.ID) {
		t.Fatal("channel mute leaked into another channel")
	}
	if err := state.SetAdminMuted(session.ID, true); err != nil {
		t.Fatal(err)
	}
	if state.CanSpeak(session.ID) {
		t.Fatal("server mute did not remain effective after a move")
	}
	user, _ := state.User(session.ID)
	if !user.ServerMuted || user.ChannelMuted || !user.AdminMuted {
		t.Fatalf("unexpected mute wire state after move: %+v", user)
	}
}

func TestChannelUserLimitRejectsJoinAndMoveWithoutChangingCurrentRoom(t *testing.T) {
	cfg := config.Default()
	cfg.Channels[0].UserLimit = 1
	state := New(cfg)
	first := state.AddSession("first", nil)
	second := state.AddSession("second", nil)
	if err := state.Join(first.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(second.ID, 2); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(second.ID, 1); !errors.Is(err, ErrChannelFull) {
		t.Fatalf("expected channel full, got %v", err)
	}
	if channel := state.ChannelOf(second.ID); channel != 2 {
		t.Fatalf("failed move changed current room to %d", channel)
	}
	if err := state.Join(first.ID, 1); err != nil {
		t.Fatalf("joining the already occupied room again must be idempotent: %v", err)
	}
}

func TestDisplayNameUpdateAppliesToEverySessionOfIdentity(t *testing.T) {
	state := New(config.Default())
	first := state.AddAuthenticatedSession("old", "client", "USER", "", nil, nil, nil)
	second := state.AddAuthenticatedSession("old", "client", "USER", "", nil, nil, nil)
	state.UpdateDisplayName("client", "new")
	for _, id := range []uint64{first.ID, second.ID} {
		user, ok := state.User(id)
		if !ok || user.Nickname != "new" {
			t.Fatalf("session %d did not receive display-name update: %+v", id, user)
		}
	}
}

func TestVoiceRecipientsAreChannelIsolated(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	c := state.AddSession("C", nil)
	for _, item := range []struct{ id, channel uint64 }{{a.ID, 2}, {b.ID, 2}, {c.ID, 1}} {
		if err := state.Join(item.id, item.channel); err != nil {
			t.Fatal(err)
		}
	}
	addresses := map[uint64]*net.UDPAddr{a.ID: {IP: net.ParseIP("127.0.0.1"), Port: 1001}, b.ID: {IP: net.ParseIP("127.0.0.1"), Port: 1002}, c.ID: {IP: net.ParseIP("127.0.0.1"), Port: 1003}}
	for id, address := range addresses {
		if err := state.RegisterUDP(id, address); err != nil {
			t.Fatal(err)
		}
	}
	recipients := state.VoiceRecipients(a.ID)
	if len(recipients) != 1 || recipients[0].Port != 1002 {
		t.Fatalf("unexpected recipients: %v", recipients)
	}
}

func TestValidateUDPRejectsWrongSourceAndSession(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	registered := &net.UDPAddr{IP: net.ParseIP("10.0.0.2"), Port: 5000}
	if err := state.RegisterUDP(session.ID, registered); err != nil {
		t.Fatal(err)
	}
	if _, ok := state.ValidateUDP(session.ID, registered); !ok {
		t.Fatal("registered source rejected")
	}
	if _, ok := state.ValidateUDP(session.ID, &net.UDPAddr{IP: registered.IP, Port: 5001}); ok {
		t.Fatal("wrong source accepted")
	}
	if _, ok := state.ValidateUDP(session.ID+1, registered); ok {
		t.Fatal("unknown session accepted")
	}
	if _, validation := state.ValidateUDPSource(session.ID, registered); validation != UDPSourceValid {
		t.Fatalf("registered endpoint classification = %v", validation)
	}
	if _, validation := state.ValidateUDPSource(session.ID, &net.UDPAddr{IP: registered.IP, Port: 5001}); validation != UDPSourceMismatch {
		t.Fatalf("wrong endpoint classification = %v", validation)
	}
	unregistered := state.AddSession("Unregistered", nil)
	if _, validation := state.ValidateUDPSource(unregistered.ID, registered); validation != UDPSourceUnregistered {
		t.Fatalf("unregistered endpoint classification = %v", validation)
	}
	if _, validation := state.ValidateUDPSource(session.ID+1, registered); validation != UDPSourceUnknownSession {
		t.Fatalf("unknown session classification = %v", validation)
	}
}

func TestRefreshUDPHandlesNATRebindingWithoutCandidateLeak(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	first := &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 5000}
	second := &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 6000}
	changed, err := state.RefreshUDP(session.ID, first)
	if err != nil || !changed {
		t.Fatalf("initial registration failed: changed=%v err=%v", changed, err)
	}
	changed, err = state.RefreshUDP(session.ID, first)
	if err != nil || changed {
		t.Fatalf("unchanged keepalive mutated state: changed=%v err=%v", changed, err)
	}
	changed, err = state.RefreshUDP(session.ID, second)
	if err != nil || !changed {
		t.Fatalf("NAT rebind was ignored: changed=%v err=%v", changed, err)
	}
	if _, ok := state.ValidateUDP(session.ID, first); ok {
		t.Fatal("stale endpoint remained valid")
	}
	if _, ok := state.ValidateUDP(session.ID, second); !ok {
		t.Fatal("rebound endpoint was rejected")
	}
	snapshot, _ := state.CandidateSnapshot(session.ID)
	if len(snapshot.Candidates) != 1 || snapshot.Candidates[0].Address != second.String() {
		t.Fatalf("stale reflexive candidates leaked: %+v", snapshot.Candidates)
	}
}

func TestSetCandidatesAcceptsOnlyBoundedNumericAddresses(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	candidates := []protocol.Candidate{
		{Kind: "host", Address: "localhost:4000", Priority: 999},
		{Kind: "ipv6", Address: "192.0.2.1:4000", Priority: 999},
	}
	for i := 1; i <= 40; i++ {
		candidates = append(candidates, protocol.Candidate{
			Kind: "host", Address: fmt.Sprintf("192.0.2.%d:%d", i, 4000+i), Priority: 400,
		})
	}
	if err := state.SetCandidates(session.ID, candidates); err != nil {
		t.Fatal(err)
	}
	snapshot, _ := state.CandidateSnapshot(session.ID)
	if len(snapshot.Candidates) != maxHostCandidates {
		t.Fatalf("candidate limit not enforced: got %d", len(snapshot.Candidates))
	}
	for _, candidate := range snapshot.Candidates {
		if candidate.Address == "localhost:4000" || candidate.Kind != "host" {
			t.Fatalf("invalid candidate accepted: %+v", candidate)
		}
	}
}

func TestUDPRebindingInvalidatesQualifiedRouteUntilPeersRequalify(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	state.mu.Lock()
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	state.mu.Unlock()
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	if route := state.EvaluateRoom(1); route.Transport != "p2p" {
		t.Fatalf("setup did not qualify: %+v", route)
	}
	if changed, err := state.RefreshUDP(a.ID, &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 62000}); err != nil || !changed {
		t.Fatalf("NAT rebind failed: changed=%v err=%v", changed, err)
	}
	if route := state.EvaluateRoom(1); route.Transport == "p2p" {
		t.Fatalf("UDP rebind kept a stale qualified P2P route: %+v", route)
	}
}

func TestClaimCandidateNoticeRateLimitsEachSession(t *testing.T) {
	state := newTestState()
	session := state.AddSession("Alice", nil)
	start := time.Unix(1_700_000_000, 0)
	interval := 500 * time.Millisecond
	if !state.ClaimCandidateNotice(session.ID, start, interval) {
		t.Fatal("first candidate notice was suppressed")
	}
	if state.ClaimCandidateNotice(session.ID, start.Add(interval-time.Nanosecond), interval) {
		t.Fatal("candidate notice inside interval was allowed")
	}
	if !state.ClaimCandidateNotice(session.ID, start.Add(interval), interval) {
		t.Fatal("candidate notice at interval boundary was suppressed")
	}
	if state.ClaimCandidateNotice(session.ID+1, start, interval) {
		t.Fatal("unknown session claimed a candidate notice")
	}
}

func TestCandidateExchangeIsLimitedToP2PEligibleRoomSizes(t *testing.T) {
	state := newTestState()
	ids := make([]uint64, 0, 7)
	for index := 0; index < 7; index++ {
		session := state.AddSession(fmt.Sprintf("user-%d", index), nil)
		if err := state.Join(session.ID, 1); err != nil {
			t.Fatal(err)
		}
		ids = append(ids, session.ID)
		if got := state.PeerCandidateFanoutAllowed(1); got != (index >= 1 && index < 6) {
			t.Fatalf("candidate eligibility with %d users = %v", index+1, got)
		}
	}
	if err := state.Leave(ids[6]); err != nil {
		t.Fatal(err)
	}
	if !state.PeerCandidateFanoutAllowed(1) {
		t.Fatal("candidate exchange did not resume after shrinking to P2P capacity")
	}
	if err := state.SetRoomNetworkPolicyForChannel(1, "force_sfu"); err != nil {
		t.Fatal(err)
	}
	if !state.PeerCandidateFanoutAllowed(1) {
		t.Fatal("small forced-SFU room lost candidate wire compatibility")
	}
	if got := state.RoomSessionIDs(1); len(got) != 6 {
		t.Fatalf("room session count = %d, want 6", len(got))
	}
}

func TestSFUFirstRoomRequiresFullQualificationMatrix(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		if err := state.Join(id, 1); err != nil {
			t.Fatal(err)
		}
	}
	initial := state.EvaluateRoom(1)
	if initial.Transport != "sfu" || initial.Topology != "p2p_evaluating" {
		t.Fatalf("room did not remain on SFU during qualification: %+v", initial)
	}
	state.mu.Lock()
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	state.mu.Unlock()
	goodAB := protocol.NetworkReport{PeerID: b.ID, Qualified: true, RTTMS: 20, Loss: 0.1, JitterMS: 2}
	goodBA := protocol.NetworkReport{PeerID: a.ID, Qualified: true, RTTMS: 21, Loss: 0.1, JitterMS: 2}
	if err := state.SetNetworkReport(a.ID, goodAB); err != nil {
		t.Fatal(err)
	}
	if partial := state.EvaluateRoom(1); partial.Transport != "sfu" {
		t.Fatalf("partial matrix migrated early: %+v", partial)
	}
	if err := state.SetNetworkReport(b.ID, goodBA); err != nil {
		t.Fatal(err)
	}
	if qualified := state.EvaluateRoom(1); qualified.Transport != "p2p" || qualified.Topology != "p2p_stable" {
		t.Fatalf("full matrix did not migrate: %+v", qualified)
	}
}

func TestP2PRoomUsesHybridForJoinAndFallsBackFastOnFailure(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	if got := state.EvaluateRoom(1); got.Transport != "p2p" {
		t.Fatalf("setup failed: %+v", got)
	}
	c := state.AddSession("C", nil)
	_ = state.Join(c.ID, 1)
	if got := state.EvaluateRoom(1); got.Transport != "hybrid" || got.Topology != "transitional_hybrid" {
		t.Fatalf("join did not preserve old routes: %+v", got)
	}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: false, Loss: 100})
	if got := state.EvaluateRoom(1); got.Transport != "sfu" || got.Reason != "p2p_degraded" {
		t.Fatalf("failure did not restore SFU: %+v", got)
	}
}

func TestHybridMembershipTransitionTimesOutToSFU(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	state.roomTopology[1] = roomRoutingState{Topology: "p2p_stable", LastChanged: time.Now().Add(-time.Minute)}
	c := state.AddSession("C", nil)
	_ = state.Join(c.ID, 1)
	if got := state.EvaluateRoom(1); got.Transport != "hybrid" {
		t.Fatalf("membership change did not enter hybrid: %+v", got)
	}
	state.mu.Lock()
	transition := state.roomTopology[1]
	transition.CooldownUntil = time.Now().Add(-time.Millisecond)
	state.roomTopology[1] = transition
	state.mu.Unlock()
	if got := state.EvaluateRoom(1); got.Transport != "sfu" || got.Reason != "p2p_qualification_timeout" {
		t.Fatalf("hybrid transition did not time out to SFU: %+v", got)
	}
}

func TestActiveRoomsAreUniqueAndSorted(t *testing.T) {
	state := newTestState()
	for _, roomID := range []uint64{2, 1, 2} {
		session := state.AddSession("member", nil)
		if err := state.Join(session.ID, roomID); err != nil {
			t.Fatal(err)
		}
	}
	if got := state.ActiveRooms(); !reflect.DeepEqual(got, []uint64{1, 2}) {
		t.Fatalf("unexpected active rooms: %v", got)
	}
}

func TestRoomAboveP2PLimitUsesSFU(t *testing.T) {
	state := newTestState()
	state.network.P2PMaxRoomSize = 2
	state.network.HybridMaxRoomSize = 4
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	if got := state.EvaluateRoom(1); got.Transport != "p2p" {
		t.Fatalf("setup failed: %+v", got)
	}
	c := state.AddSession("C", nil)
	_ = state.Join(c.ID, 1)
	if got := state.EvaluateRoom(1); got.Transport != "sfu" || got.Reason != "room_too_large_for_p2p" {
		t.Fatalf("oversized room did not settle on SFU: %+v", got)
	}
}

func TestForceSFUPolicyPreventsQualifiedP2PMigration(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	if err := state.SetRoomNetworkPolicy(a.ID, "force_sfu"); err != nil {
		t.Fatal(err)
	}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	got := state.EvaluateRoom(1)
	if got.Transport != "sfu" || got.Topology != "sfu_stable" || got.RoomNetworkPolicy != "force_sfu" {
		t.Fatalf("force SFU policy was not respected: %+v", got)
	}
}

func TestRoomNetworkPolicyCanTargetAChannelWithoutOperatorMembership(t *testing.T) {
	state := newTestState()
	if err := state.SetRoomNetworkPolicyForChannel(2, "force_sfu"); err != nil {
		t.Fatal(err)
	}
	if route := state.EvaluateRoom(2); route.RoomNetworkPolicy != "force_sfu" || route.Transport != "sfu" {
		t.Fatalf("targeted room policy was not applied: %+v", route)
	}
	if err := state.SetRoomNetworkPolicyForChannel(999, "auto"); err != ErrUnknownChannel {
		t.Fatalf("unknown target channel returned %v", err)
	}
}

func TestRoomNetworkPolicySurvivesAnEmptyRoom(t *testing.T) {
	state := newTestState()
	session := state.AddSession("A", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.SetRoomNetworkPolicyForChannel(1, "force_sfu"); err != nil {
		t.Fatal(err)
	}
	if err := state.Leave(session.ID); err != nil {
		t.Fatal(err)
	}
	if got := state.RoomNetworkPolicy(1); got != "force_sfu" {
		t.Fatalf("empty room lost its policy: %q", got)
	}
	if route := state.EvaluateRoom(1); route.RoomNetworkPolicy != "force_sfu" || route.Transport != "sfu" {
		t.Fatalf("empty room no longer evaluates with its policy: %+v", route)
	}
}

func TestRoomNetworkPolicyRestoresFromConfiguration(t *testing.T) {
	cfg := config.Default()
	cfg.Channels[0].NetworkPolicy = "force_sfu"
	state := New(cfg)
	if got := state.RoomNetworkPolicy(cfg.Channels[0].ID); got != "force_sfu" {
		t.Fatalf("configured policy was not restored: %q", got)
	}
	snapshot := state.Snapshot()
	if len(snapshot.Channels) == 0 || snapshot.Channels[0].NetworkPolicy != "force_sfu" {
		t.Fatalf("snapshot omitted the authoritative policy: %+v", snapshot.Channels)
	}
}

func TestForceP2PUsesTemporarySFUWithoutChangingPolicy(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	for _, id := range []uint64{a.ID, b.ID} {
		_ = state.Join(id, 1)
	}
	if err := state.SetRoomNetworkPolicy(a.ID, "force_p2p"); err != nil {
		t.Fatal(err)
	}
	pending := state.EvaluateRoom(1)
	if pending.Transport != "sfu" || pending.Topology != "p2p_pending" || pending.RoomNetworkPolicy != "force_p2p" {
		t.Fatalf("force P2P did not retain the SFU admission baseline: %+v", pending)
	}
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	if got := state.EvaluateRoom(1); got.Transport != "p2p" || got.RoomNetworkPolicy != "force_p2p" {
		t.Fatalf("force P2P did not converge to P2P: %+v", got)
	}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: false, Loss: 100})
	recovery := state.EvaluateRoom(1)
	if recovery.Transport != "sfu" || recovery.Reason != "temporary_sfu_recovery" || recovery.RoomNetworkPolicy != "force_p2p" {
		t.Fatalf("temporary SFU recovery changed policy: %+v", recovery)
	}
}

func TestForceP2PHardLimitIsExplicit(t *testing.T) {
	state := newTestState()
	state.network.P2PMaxRoomSize = 2
	var first uint64
	for index := 0; index < 3; index++ {
		session := state.AddSession(fmt.Sprintf("user-%d", index), nil)
		if index == 0 {
			first = session.ID
		}
		_ = state.Join(session.ID, 1)
	}
	if err := state.SetRoomNetworkPolicy(first, "force_p2p"); err != ErrP2PNotSupported {
		t.Fatalf("expected explicit P2P guardrail error, got %v", err)
	}
}

func TestExplicitSubscriptionsStayInsideRoom(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	c := state.AddSession("C", nil)
	_ = state.Join(a.ID, 1)
	_ = state.Join(b.ID, 1)
	_ = state.Join(c.ID, 2)
	for i, id := range []uint64{a.ID, b.ID, c.ID} {
		_ = state.RegisterUDP(id, &net.UDPAddr{IP: net.ParseIP("127.0.0.1"), Port: 2000 + i})
	}
	if err := state.SetSubscriptions(b.ID, []uint64{a.ID}); err != nil {
		t.Fatal(err)
	}
	if got := state.VoiceRecipients(a.ID); len(got) != 1 || got[0].Port != 2001 {
		t.Fatalf("subscription mismatch: %v", got)
	}
	if err := state.SetSubscriptions(b.ID, []uint64{c.ID}); err == nil {
		t.Fatal("cross-room subscription accepted")
	}
}

func TestNetworkReportsAreBoundedToCurrentRoom(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	c := state.AddSession("C", nil)
	_ = state.Join(a.ID, 1)
	_ = state.Join(b.ID, 1)
	_ = state.Join(c.ID, 2)

	if err := state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, RTTMS: 20, Loss: 1, JitterMS: 2}); err != nil {
		t.Fatal(err)
	}
	if err := state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: c.ID}); err == nil {
		t.Fatal("cross-room network report accepted")
	}
	if err := state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: 999_999}); err == nil {
		t.Fatal("unknown network report target accepted")
	}
	if err := state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Loss: 101}); err == nil {
		t.Fatal("invalid network metrics accepted")
	}
	state.mu.RLock()
	reportCount := len(state.sessions[a.ID].Reports)
	state.mu.RUnlock()
	if reportCount != 1 {
		t.Fatalf("unexpected report count: %d", reportCount)
	}
}

func TestMovingOrLeavingClearsPeerReportsAndSubscriptions(t *testing.T) {
	state := newTestState()
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	_ = state.Join(a.ID, 1)
	_ = state.Join(b.ID, 1)
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID})
	_ = state.SetSubscriptions(a.ID, []uint64{b.ID})

	if err := state.Join(b.ID, 2); err != nil {
		t.Fatal(err)
	}
	state.mu.RLock()
	_, aReportedB := state.sessions[a.ID].Reports[b.ID]
	_, aSubscribedB := state.sessions[a.ID].Subscriptions[b.ID]
	bReportCount := len(state.sessions[b.ID].Reports)
	state.mu.RUnlock()
	if aReportedB || aSubscribedB || bReportCount != 0 {
		t.Fatal("moving peer retained stale reports or subscriptions")
	}

	if err := state.Leave(a.ID); err != nil {
		t.Fatal(err)
	}
	state.mu.RLock()
	aReportCount := len(state.sessions[a.ID].Reports)
	aSubscriptionCount := len(state.sessions[a.ID].Subscriptions)
	state.mu.RUnlock()
	if aReportCount != 0 || aSubscriptionCount != 0 {
		t.Fatal("leaving peer retained room-scoped state")
	}
}

func TestChannelChatHistoryIsScopedBoundedAndServerAttributed(t *testing.T) {
	state := newTestState()
	if empty := state.ChatHistory(1); empty == nil || len(empty) != 0 {
		t.Fatalf("empty chat history must be a non-nil JSON sequence: %#v", empty)
	}
	alice := state.AddAuthenticatedSession("Alice", "client-a", "USER", "", nil, nil, nil)
	bob := state.AddSession("Bob", nil)
	_ = state.Join(alice.ID, 1)
	_ = state.Join(bob.ID, 2)
	_ = state.SetAvatar(alice.ID, "L")
	base := time.Now().Add(-time.Minute)
	message, err := state.RecordTextMessage(alice.ID, "hello", base)
	if err != nil {
		t.Fatal(err)
	}
	if message.ChannelID != 1 || message.SenderSessionID != alice.ID || message.SenderClientID != "client-a" || message.SenderName != "Alice" || message.SenderAvatar != "L" {
		t.Fatalf("message metadata was not server attributed: %+v", message)
	}
	if got := state.ChatHistory(2); len(got) != 0 {
		t.Fatalf("chat leaked across channels: %+v", got)
	}
	for index := 0; index < 105; index++ {
		if _, err := state.RecordTextMessage(alice.ID, "bounded", base.Add(time.Duration(index+1)*time.Second)); err != nil {
			t.Fatal(err)
		}
	}
	history := state.ChatHistory(1)
	if len(history) != 100 || history[0].ID == message.ID {
		t.Fatalf("history was not bounded to newest 100 messages: len=%d first=%d", len(history), history[0].ID)
	}
}

func TestTemporaryChannelCanOnlyBeDeletedAfterItBecomesEmpty(t *testing.T) {
	state := newTestState()
	if err := state.CreateChannel(protocol.Channel{ID: 99, Name: "Quick room", Temporary: true, AudioQualityPolicy: "quality_7"}); err != nil {
		t.Fatal(err)
	}
	user := state.AddSession("Alice", nil)
	if err := state.Join(user.ID, 99); err != nil {
		t.Fatal(err)
	}
	if _, deleted := state.DeleteEmptyTemporaryChannel(99); deleted {
		t.Fatal("occupied temporary channel was deleted")
	}
	if err := state.Leave(user.ID); err != nil {
		t.Fatal(err)
	}
	channel, deleted := state.DeleteEmptyTemporaryChannel(99)
	if !deleted || channel.AudioQualityPolicy != "quality_7" {
		t.Fatalf("empty temporary channel was not deleted: deleted=%v channel=%+v", deleted, channel)
	}
	if _, exists := state.DeleteEmptyTemporaryChannel(99); exists {
		t.Fatal("temporary channel was deleted twice")
	}
}

func TestExpiredQualificationFallsBackAndRequiresFreshReports(t *testing.T) {
	state := newTestState()
	a, b := state.AddSession("A", nil), state.AddSession("B", nil)
	_ = state.Join(a.ID, 1)
	_ = state.Join(b.ID, 1)
	state.roomTopology[1] = roomRoutingState{Topology: "sfu_stable", LastChanged: time.Now().Add(-time.Minute)}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	_ = state.SetNetworkReport(b.ID, protocol.NetworkReport{PeerID: a.ID, Qualified: true})
	if got := state.EvaluateRoom(1); got.Transport != "p2p" {
		t.Fatal(got)
	}
	state.sessions[a.ID].ReportTimes[b.ID] = time.Now().Add(-4 * time.Second)
	if got := state.EvaluateRoom(1); got.Transport != "sfu" || got.Reason != "p2p_degraded" {
		t.Fatal(got)
	}
	_ = state.SetNetworkReport(a.ID, protocol.NetworkReport{PeerID: b.ID, Qualified: true})
	if got := state.EvaluateRoom(1); got.Transport != "sfu" || got.Reason != "migration_cooldown" {
		t.Fatal(got)
	}
}

func TestFullyQualifiedHybridCompletesWithoutAnExtraSFUHop(t *testing.T) {
	state := newTestState()
	ids := []uint64{}
	for _, name := range []string{"A", "B", "C"} {
		x := state.AddSession(name, nil)
		_ = state.Join(x.ID, 1)
		ids = append(ids, x.ID)
	}
	state.roomTopology[1] = roomRoutingState{Topology: "transitional_hybrid", LastChanged: time.Now(), CooldownUntil: time.Now().Add(time.Second)}
	for _, a := range ids {
		for _, b := range ids {
			if a != b {
				_ = state.SetNetworkReport(a, protocol.NetworkReport{PeerID: b, Qualified: true})
			}
		}
	}
	if got := state.EvaluateRoom(1); got.Transport != "p2p" {
		t.Fatalf("unnecessary SFU hop: %+v", got)
	}
}
