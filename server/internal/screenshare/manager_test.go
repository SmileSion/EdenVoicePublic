package screenshare

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"
)

func testManager(t *testing.T) (*Manager, func(uint64, uint64)) {
	t.Helper()
	cfg := DefaultConfig()
	cfg.Enabled = true
	cfg.UDPPort = unusedUDPPort(t)
	cfg.PublicIP = "127.0.0.1"
	var mu sync.RWMutex
	members := map[uint64]uint64{1: 7, 2: 7, 3: 8}
	m, err := New(cfg, Hooks{Allowed: func(id, room uint64) bool { mu.RLock(); defer mu.RUnlock(); return members[id] == room && room != 0 }})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(m.Close)
	return m, func(id, room uint64) { mu.Lock(); members[id] = room; mu.Unlock() }
}
func testPeer(t *testing.T) *webrtc.PeerConnection {
	t.Helper()
	settings := webrtc.SettingEngine{}
	settings.SetIncludeLoopbackCandidate(true)
	pc, err := webrtc.NewAPI(webrtc.WithSettingEngine(settings)).NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = pc.Close() })
	return pc
}
func offer(t *testing.T, pc *webrtc.PeerConnection) string {
	t.Helper()
	sdp, err := pc.CreateOffer(nil)
	if err != nil {
		t.Fatal(err)
	}
	done := webrtc.GatheringCompletePromise(pc)
	if err = pc.SetLocalDescription(sdp); err != nil {
		t.Fatal(err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("gather timed out")
	}
	return pc.LocalDescription().SDP
}
func wait(t *testing.T, f func() bool) {
	t.Helper()
	until := time.Now().Add(10 * time.Second)
	for time.Now().Before(until) {
		if f() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("condition did not become true")
}
func TestPolicyAndActualMediaLimits(t *testing.T) {
	cfg := DefaultConfig()
	cfg.Enabled = true
	good := Settings{Width: 1280, Height: 720, FPS: 30, BitrateKbps: 2000, Audio: true}
	if err := good.Validate(cfg.Policy); err != nil {
		t.Fatal(err)
	}
	bad := good
	bad.BitrateKbps = cfg.MaxBitrateKbps + 1
	if bad.Validate(cfg.Policy) == nil {
		t.Fatal("bitrate accepted above cap")
	}
	cfg.AllowAudio = false
	if good.Validate(cfg.Policy) == nil {
		t.Fatal("forbidden audio accepted")
	}
	now := time.Now()
	packet := func(width, height int) *rtp.Packet {
		return &rtp.Packet{Header: rtp.Header{Version: 2}, Payload: []byte{0x10, 0, 0, 0, 0x9d, 1, 0x2a, byte(width), byte(width >> 8), byte(height), byte(height >> 8)}}
	}
	if !newMediaGuard(good, true, now).accept(packet(1280, 720), now) {
		t.Fatal("valid VP8 keyframe rejected")
	}
	if newMediaGuard(good, true, now).accept(packet(1920, 1080), now) {
		t.Fatal("undeclared resolution accepted")
	}
	g := newMediaGuard(good, true, now)
	accepted := true
	for i := 0; i < 300; i++ {
		if !g.accept(packet(1280, 720), now.Add(time.Duration(i)*10*time.Millisecond)) {
			accepted = false
			break
		}
	}
	if accepted {
		t.Fatal("100fps accepted with a 30fps limit")
	}
	g = newMediaGuard(good, false, now)
	large := &rtp.Packet{Payload: make([]byte, 1200)}
	accepted = true
	for i := 0; i < 100; i++ {
		if !g.accept(large, now) {
			accepted = false
			break
		}
	}
	if accepted {
		t.Fatal("unbounded audio burst accepted")
	}
}
func TestVideoAndAudioRelayRoomIsolationAndCleanup(t *testing.T) {
	m, move := testManager(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	publisher := testPeer(t)
	video, _ := webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000}, "video", "screen")
	audio, _ := webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeOpus, ClockRate: 48000, Channels: 2}, "audio", "screen")
	for _, track := range []*webrtc.TrackLocalStaticRTP{video, audio} {
		sender, err := publisher.AddTrack(track)
		if err != nil {
			t.Fatal(err)
		}
		go func() {
			for {
				if _, _, err := sender.ReadRTCP(); err != nil {
					return
				}
			}
		}()
	}
	settings := Settings{Width: 1280, Height: 720, FPS: 30, BitrateKbps: 2000, Audio: true}
	state, answer, err := m.Publish(ctx, 1, 7, settings, offer(t, publisher))
	if err != nil {
		t.Fatal(err)
	}
	if err = publisher.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeAnswer, SDP: answer}); err != nil {
		t.Fatal(err)
	}
	wait(t, func() bool { return publisher.ConnectionState() == webrtc.PeerConnectionStateConnected })
	// Synthetic encoded-payload transport fixture; pixel rendering is tested by
	// the browser end-to-end suite, not inferred from receipt of these RTP bytes.
	var seq uint16
	send := func() {
		seq++
		_ = video.WriteRTP(&rtp.Packet{Header: rtp.Header{Version: 2, SequenceNumber: seq, Timestamp: uint32(seq) * 3000, Marker: true}, Payload: []byte{0x10, 1, 2, 3}})
		_ = audio.WriteRTP(&rtp.Packet{Header: rtp.Header{Version: 2, SequenceNumber: seq, Timestamp: uint32(seq) * 960}, Payload: []byte{0xf8, 0xff, 0xfe}})
	}
	send()
	wait(t, func() bool { return m.Status(7).Ready })
	if _, _, err = m.Subscribe(ctx, 3, 7, state.ShareID, "invalid"); err == nil {
		t.Fatal("cross-room viewer allowed")
	}
	if err = m.Stop(2, 7, state.ShareID, false); err == nil {
		t.Fatal("ordinary viewer can stop share")
	}
	if err = m.Pause(1, 7, state.ShareID, true); err != nil {
		t.Fatal(err)
	}
	if !m.Status(7).Paused {
		t.Fatal("privacy pause missing from room status")
	}
	viewer := testPeer(t)
	_, _ = viewer.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionRecvonly})
	_, _ = viewer.AddTransceiverFromKind(webrtc.RTPCodecTypeAudio, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionRecvonly})
	received := make(chan webrtc.RTPCodecType, 8)
	viewer.OnTrack(func(track *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		if _, _, err := track.ReadRTP(); err == nil {
			received <- track.Kind()
		}
	})
	subscription, answer, err := m.Subscribe(ctx, 2, 7, state.ShareID, offer(t, viewer))
	if err != nil {
		t.Fatal(err)
	}
	if !subscription.Paused {
		t.Fatal("late viewer did not receive paused state")
	}
	if err = viewer.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeAnswer, SDP: answer}); err != nil {
		t.Fatal(err)
	}
	wait(t, func() bool { return viewer.ConnectionState() == webrtc.PeerConnectionStateConnected })
	seen := map[webrtc.RTPCodecType]bool{}
	for i := 0; i < 100 && len(seen) < 2; i++ {
		send()
		select {
		case kind := <-received:
			seen[kind] = true
		case <-time.After(35 * time.Millisecond):
		}
	}
	if !seen[webrtc.RTPCodecTypeVideo] || !seen[webrtc.RTPCodecTypeAudio] {
		t.Fatal("both video and audio must traverse SFU", seen)
	}
	// Privacy video/audio replacement is sent by the publisher. The SFU must
	// continue forwarding while paused so older clients cannot freeze real pixels.
	if err = m.Pause(1, 7, state.ShareID, false); err != nil {
		t.Fatal(err)
	}
	if m.Status(7).Paused {
		t.Fatal("privacy resume missing from room status")
	}
	move(2, 8)
	m.RemoveSession(2)
	if m.Status(7).Viewers != 0 {
		t.Fatal("departed viewer retained")
	}
	if err = m.Stop(1, 7, "stale-share", false); err == nil {
		t.Fatal("stale share ID accepted")
	}
	if err = m.Stop(1, 7, state.ShareID, false); err != nil {
		t.Fatal(err)
	}
	if m.Status(7).ShareID != "" {
		t.Fatal("stopped share remains")
	}
}
func TestRejectedOfferDoesNotReserveRoom(t *testing.T) {
	m, _ := testManager(t)
	_, _, err := m.Publish(context.Background(), 1, 7, Settings{Width: 1280, Height: 720, FPS: 30, BitrateKbps: 2000}, "not SDP")
	if err == nil || m.Status(7).ShareID != "" {
		t.Fatal("invalid negotiation leaked room")
	}
}

func TestBrowserPaddingIsNotMalformedVideo(t *testing.T) {
	now := time.Now()
	settings := Settings{Width: 640, Height: 360, FPS: 10, BitrateKbps: 1000}
	g := newMediaGuard(settings, true, now)
	probe := &rtp.Packet{Header: rtp.Header{Version: 2, Padding: true}, PaddingSize: 255}
	if !g.accept(probe, now) {
		t.Fatal("legal browser bandwidth probe rejected")
	}
	if g.frames != 0 {
		t.Fatal("padding counted as a video frame")
	}
	if g.accept(&rtp.Packet{Header: rtp.Header{Version: 2}}, now) {
		t.Fatal("empty unpadded video accepted")
	}
	limited := false
	for i := 0; i < 2000; i++ {
		if !g.accept(probe, now) {
			limited = true
			break
		}
	}
	if !limited {
		t.Fatal("padding bypassed bitrate budget")
	}
}

func startPolicyTestShare(t *testing.T, m *Manager) State {
	t.Helper()
	publisher := testPeer(t)
	track, err := webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000}, "video", "screen")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = publisher.AddTrack(track); err != nil {
		t.Fatal(err)
	}
	state, _, err := m.Publish(context.Background(), 1, 7, Settings{Width: 640, Height: 360, FPS: 10, BitrateKbps: 1000}, offer(t, publisher))
	if err != nil {
		t.Fatal(err)
	}
	return state
}
func TestPolicyChangeClosesActiveShareAndRejectsDisabledPublishing(t *testing.T) {
	m, _ := testManager(t)
	state := startPolicyTestShare(t, m)
	m.mu.RLock()
	endpoint := m.rooms[7].publisher
	m.mu.RUnlock()
	policy := m.Policy()
	policy.Enabled = false
	if err := m.SetPolicy(policy); err != nil {
		t.Fatal(err)
	}
	select {
	case <-endpoint.done:
	default:
		t.Fatal("policy disable retained publisher")
	}
	after := m.Status(7)
	if after.ShareID != "" || after.Policy.Enabled || after.Revision <= state.Revision {
		t.Fatal("disabled policy not reflected in fresh room state")
	}
	_, _, err := m.Publish(context.Background(), 1, 7, Settings{Width: 640, Height: 360, FPS: 10, BitrateKbps: 1000}, "unused")
	if err == nil || m.Status(7).ShareID != "" {
		t.Fatal("disabled sharing created a room")
	}
}
func TestInvalidPolicyDoesNotInterruptActiveShare(t *testing.T) {
	m, _ := testManager(t)
	state := startPolicyTestShare(t, m)
	before := m.Policy()
	invalid := before
	invalid.MaxFPS = 0
	if m.SetPolicy(invalid) == nil {
		t.Fatal("invalid fps accepted")
	}
	if m.Status(7).ShareID != state.ShareID || m.Policy() != before {
		t.Fatal("invalid update changed active share")
	}
}
func TestCancelledPublishDoesNotReserveRoom(t *testing.T) {
	m, _ := testManager(t)
	publisher := testPeer(t)
	_, err := publisher.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionSendonly})
	if err != nil {
		t.Fatal(err)
	}
	sdp := offer(t, publisher)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, _, err = m.Publish(ctx, 1, 7, Settings{Width: 640, Height: 360, FPS: 10, BitrateKbps: 1000}, sdp)
	if err == nil || m.Status(7).ShareID != "" {
		t.Fatal("cancelled publish retained a room")
	}
}

func TestSavingUnchangedPolicyKeepsActiveShare(t *testing.T) {
	m, _ := testManager(t)
	state := startPolicyTestShare(t, m)
	m.mu.RLock()
	endpoint := m.rooms[7].publisher
	m.mu.RUnlock()
	if err := m.SetPolicy(m.Policy()); err != nil {
		t.Fatal(err)
	}
	if m.Status(7).ShareID != state.ShareID {
		t.Fatal("saving unchanged policy stopped sharing")
	}
	select {
	case <-endpoint.done:
		t.Fatal("unchanged policy closed connection")
	default:
	}
}

func TestIndependentRoomsAndSinglePublisher(t *testing.T) {
	m, _ := testManager(t)
	first := startPolicyTestShare(t, m)
	settings := Settings{Width: 640, Height: 360, FPS: 10, BitrateKbps: 1000}
	if _, _, err := m.Publish(context.Background(), 2, 7, settings, "unused"); err == nil {
		t.Fatal("second publisher replaced room host")
	}
	peer := testPeer(t)
	_, err := peer.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionSendonly})
	if err != nil {
		t.Fatal(err)
	}
	second, _, err := m.Publish(context.Background(), 3, 8, settings, offer(t, peer))
	if err != nil {
		t.Fatal(err)
	}
	if first.ShareID == second.ShareID {
		t.Fatal("shared identity across rooms")
	}
	if err := m.Stop(3, 7, first.ShareID, true); err == nil {
		t.Fatal("foreign room moderator can stop share")
	}
	if err := m.Stop(1, 7, first.ShareID, false); err != nil {
		t.Fatal(err)
	}
	if m.Status(8).ShareID != second.ShareID {
		t.Fatal("stopping first room affected second room")
	}
	m.RemoveSession(3)
	if m.Status(8).ShareID != "" {
		t.Fatal("departed publisher retained")
	}
}

func TestPrivacyPauseOnlyCurrentPublisherAndShareCanChangeState(t *testing.T) {
	members := map[uint64]uint64{1: 7, 2: 7, 3: 8}
	m := &Manager{rooms: map[uint64]*room{7: {id: "share-one", channel: 7, host: 1}}, hooks: Hooks{Allowed: func(session, channel uint64) bool { return channel != 0 && members[session] == channel }}}
	notifications := 0
	m.hooks.Changed = func(channel uint64, _ string) {
		notifications++
		if m.Status(channel).ShareID != "share-one" {
			t.Fatal("notification lost current state")
		}
	}
	if m.Status(7).Paused {
		t.Fatal("new share started paused")
	}
	for _, tc := range []struct {
		session, channel uint64
		share            string
	}{{2, 7, "share-one"}, {3, 7, "share-one"}, {1, 8, "share-one"}, {1, 7, "stale-share"}} {
		if err := m.Pause(tc.session, tc.channel, tc.share, true); err == nil {
			t.Fatalf("unauthorized pause accepted: %+v", tc)
		}
	}
	if notifications != 0 || m.Status(7).Paused {
		t.Fatal("rejected pause changed state")
	}
	if err := m.Pause(1, 7, "share-one", true); err != nil {
		t.Fatal(err)
	}
	if !m.Status(7).Paused || notifications != 1 {
		t.Fatal("pause did not broadcast")
	}
	if err := m.Pause(1, 7, "share-one", true); err != nil || notifications != 1 {
		t.Fatal("same pause state broadcast repeatedly")
	}
	members[1] = 8
	if err := m.Pause(1, 7, "share-one", false); err == nil || !m.Status(7).Paused {
		t.Fatal("departed publisher changed paused state")
	}
	members[1] = 7
	if err := m.Pause(1, 7, "share-one", false); err != nil || m.Status(7).Paused || notifications != 2 {
		t.Fatal("resume not broadcast")
	}
	m.rooms[7] = &room{id: "share-two", channel: 7, host: 1}
	if m.Status(7).Paused {
		t.Fatal("new share inherited privacy state")
	}
}
