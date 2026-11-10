package main

import (
	"bytes"
	"fmt"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"
	"github.com/pion/ice/v4"
	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"
)

// Opt-in deployment check using authenticated TLS signaling and real SRTP.
// It sends synthetic RTP bytes, not captured video/audio or a decodable movie.
// Choose an unused room; an existing share is never stopped or replaced.
func TestLiveScreenShareFixedPort(t *testing.T) {
	addr := os.Getenv("EDENVOICE_SCREEN_CONTROL")
	if addr == "" {
		t.Skip("live screen sharing check is opt-in")
	}
	channel, err := strconv.ParseUint(os.Getenv("EDENVOICE_SCREEN_CHANNEL"), 10, 64)
	if err != nil || channel == 0 {
		t.Fatal("EDENVOICE_SCREEN_CHANNEL must identify an unused voice room")
	}
	port, err := strconv.Atoi(os.Getenv("EDENVOICE_SCREEN_PORT"))
	if err != nil || port < 1 || port > 65535 {
		t.Fatal("EDENVOICE_SCREEN_PORT must be the fixed server UDP port")
	}
	opts := options{controlAddr: addr, fingerprint: os.Getenv("EDENVOICE_SCREEN_FINGERPRINT"), channelID: channel, timeout: 10 * time.Second}
	identity, err := loadIdentity("")
	if err != nil {
		t.Fatal(err)
	}
	clients := make([]*smokeClient, 2)
	for i := range clients {
		clients[i], err = connect(opts, identity, fmt.Sprintf("Screen deployment check %d", i+1))
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(clients[i].close)
		if err := clients[i].join(channel, opts.timeout); err != nil {
			t.Fatal(err)
		}
	}
	var requestID int
	request := func(c *smokeClient, req screenshare.Request) screenshare.Response {
		t.Helper()
		requestID++
		req.RequestID, req.ChannelID = fmt.Sprintf("deployment-%d", requestID), channel
		if err := c.send(protocol.TypeScreenShareRequest, req); err != nil {
			t.Fatal(err)
		}
		deadline := time.Now().Add(opts.timeout)
		for time.Now().Before(deadline) {
			envelope, err := c.read(time.Until(deadline))
			if err != nil {
				t.Fatal(err)
			}
			if envelope.Type != protocol.TypeScreenShareState {
				continue
			}
			var response screenshare.Response
			if err := envelope.Decode(&response); err != nil {
				t.Fatal(err)
			}
			if response.RequestID != req.RequestID {
				continue
			}
			if response.Error != "" {
				t.Fatalf("%s failed: %s", req.Action, response.Error)
			}
			return response
		}
		t.Fatal("screen share response timed out")
		return screenshare.Response{}
	}
	initial := request(clients[0], screenshare.Request{Action: "status"})
	if initial.ShareID != "" || !initial.Policy.Enabled {
		t.Fatal("live check requires an enabled, unused sharing room")
	}
	peer := func() *webrtc.PeerConnection {
		t.Helper()
		pc, err := webrtc.NewPeerConnection(webrtc.Configuration{})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = pc.Close() })
		return pc
	}
	offer := func(pc *webrtc.PeerConnection) string {
		t.Helper()
		sdp, err := pc.CreateOffer(nil)
		if err != nil {
			t.Fatal(err)
		}
		done := webrtc.GatheringCompletePromise(pc)
		if err := pc.SetLocalDescription(sdp); err != nil {
			t.Fatal(err)
		}
		select {
		case <-done:
		case <-time.After(opts.timeout):
			t.Fatal("ICE gathering timed out")
		}
		return pc.LocalDescription().SDP
	}
	answer := func(pc *webrtc.PeerConnection, sdp string) {
		t.Helper()
		candidates := 0
		for _, line := range strings.Split(sdp, "\n") {
			line = strings.TrimSpace(line)
			if !strings.HasPrefix(line, "a=candidate:") {
				continue
			}
			candidate, err := ice.UnmarshalCandidate(strings.TrimPrefix(line, "a=candidate:"))
			if err != nil || candidate.Port() != port || !candidate.NetworkType().IsUDP() {
				t.Fatalf("server candidate must use UDP port %d: %v", port, err)
			}
			candidates++
		}
		if candidates == 0 {
			t.Fatal("server returned no UDP candidates")
		}
		if err := pc.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeAnswer, SDP: sdp}); err != nil {
			t.Fatal(err)
		}
		deadline := time.Now().Add(opts.timeout)
		for time.Now().Before(deadline) {
			if pc.ConnectionState() == webrtc.PeerConnectionStateConnected {
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("WebRTC did not connect: %s", pc.ConnectionState())
	}
	publisher := peer()
	track, err := webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000}, "video", "deployment-check")
	if err != nil {
		t.Fatal(err)
	}
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
	published := request(clients[0], screenshare.Request{Action: "publish", Settings: screenshare.Settings{Width: 320, Height: 180, FPS: 1, BitrateKbps: 128}, SDP: offer(publisher)})
	answer(publisher, published.SDP)
	payload := []byte{0x10, 1, 2, 3}
	stop := make(chan struct{})
	stopped := make(chan struct{})
	t.Cleanup(func() { close(stop); <-stopped })
	go func() {
		defer close(stopped)
		ticker := time.NewTicker(time.Second)
		defer ticker.Stop()
		var seq uint16
		for {
			select {
			case <-stop:
				return
			case <-ticker.C:
				seq++
				_ = track.WriteRTP(&rtp.Packet{Header: rtp.Header{Version: 2, SequenceNumber: seq, Timestamp: uint32(seq) * 90000, Marker: true}, Payload: payload})
			}
		}
	}()
	ready := false
	for deadline := time.Now().Add(opts.timeout); time.Now().Before(deadline); {
		time.Sleep(200 * time.Millisecond)
		if request(clients[0], screenshare.Request{Action: "status"}).Ready {
			ready = true
			break
		}
	}
	if !ready {
		t.Fatal("server did not receive publisher media")
	}
	viewer := peer()
	if _, err := viewer.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionRecvonly}); err != nil {
		t.Fatal(err)
	}
	received := make(chan struct{}, 1)
	viewer.OnTrack(func(remote *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		p, _, err := remote.ReadRTP()
		if err == nil && bytes.Equal(p.Payload, payload) {
			received <- struct{}{}
		}
	})
	subscribed := request(clients[1], screenshare.Request{Action: "subscribe", ShareID: published.ShareID, SDP: offer(viewer)})
	answer(viewer, subscribed.SDP)
	select {
	case <-received:
	case <-time.After(opts.timeout):
		t.Fatal("viewer did not receive matching synthetic RTP through the server")
	}
	state := request(clients[0], screenshare.Request{Action: "stop", ShareID: published.ShareID})
	if state.ShareID != "" {
		t.Fatal("stop retained sharing state")
	}
	t.Logf("authenticated publish/view/stop passed; both sessions use server UDP %d; synthetic SRTP payload matched", port)
}
