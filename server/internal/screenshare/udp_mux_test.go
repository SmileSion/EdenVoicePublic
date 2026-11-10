package screenshare

import (
	"bytes"
	"context"
	"errors"
	"net"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/pion/rtp"
	"github.com/pion/stun/v3"
	"github.com/pion/webrtc/v4"
)

func unusedUDPPort(t *testing.T) uint16 {
	t.Helper()
	c, err := net.ListenUDP("udp4", &net.UDPAddr{Port: 0})
	if err != nil {
		t.Fatal(err)
	}
	port := uint16(c.LocalAddr().(*net.UDPAddr).Port)
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	return port
}

func assertCandidatePort(t *testing.T, answer string, port uint16) {
	t.Helper()
	count := 0
	for _, line := range strings.Split(answer, "\n") {
		if !strings.HasPrefix(line, "a=candidate:") {
			continue
		}
		fields := strings.Fields(line)
		if len(fields) < 8 || strings.ToLower(fields[2]) != "udp" || fields[5] != strconv.Itoa(int(port)) {
			t.Fatalf("candidate escaped fixed port %d: %q", port, line)
		}
		count++
	}
	if count == 0 {
		t.Fatal("answer has no candidates")
	}
}

func TestFixedUDPPortLifecycleAndPolicyFailure(t *testing.T) {
	cfg := DefaultConfig()
	cfg.UDPPort = unusedUDPPort(t)
	occupied, err := net.ListenUDP("udp4", &net.UDPAddr{Port: int(cfg.UDPPort)})
	if err != nil {
		t.Fatal(err)
	}
	defer occupied.Close()
	hooks := Hooks{Allowed: func(uint64, uint64) bool { return true }}
	m, err := New(cfg, hooks)
	if err != nil {
		t.Fatalf("disabled startup bound port: %v", err)
	}
	t.Cleanup(m.Close)
	enabled := cfg.Policy
	enabled.Enabled = true
	saved := false
	if err = m.UpdatePolicy(enabled, func() error { saved = true; return nil }); err == nil {
		t.Fatal("enable accepted occupied port")
	}
	if saved || m.Policy().Enabled {
		t.Fatal("failed bind saved or applied policy")
	}
	activeCfg := cfg
	activeCfg.Enabled = true
	if other, err := New(activeCfg, hooks); err == nil {
		other.Close()
		t.Fatal("enabled startup accepted occupied port")
	}
	if err = occupied.Close(); err != nil {
		t.Fatal(err)
	}
	storageErr := errors.New("storage unavailable")
	if err = m.UpdatePolicy(enabled, func() error { return storageErr }); !errors.Is(err, storageErr) {
		t.Fatal(err)
	}
	if m.Policy().Enabled || m.udpMux != nil {
		t.Fatal("save failure leaked enabled transport")
	}
	check, err := net.ListenUDP("udp4", &net.UDPAddr{Port: int(cfg.UDPPort)})
	if err != nil {
		t.Fatalf("save failure leaked socket: %v", err)
	}
	_ = check.Close()
	for i := 0; i < 3; i++ {
		if err = m.SetPolicy(enabled); err != nil {
			t.Fatal(err)
		}
		if m.udpMux == nil {
			t.Fatal("enable did not bind")
		}
		if err = m.SetPolicy(cfg.Policy); err != nil {
			t.Fatal(err)
		}
		check, err = net.ListenUDP("udp4", &net.UDPAddr{Port: int(cfg.UDPPort)})
		if err != nil {
			t.Fatalf("disable did not release socket: %v", err)
		}
		_ = check.Close()
	}
	if err = m.SetPolicy(enabled); err != nil {
		t.Fatal(err)
	}
	m.Close()
	m.Close()
	if err = m.SetPolicy(enabled); err == nil {
		t.Fatal("closed manager re-enabled")
	}
	check, err = net.ListenUDP("udp4", &net.UDPAddr{Port: int(cfg.UDPPort)})
	if err != nil {
		t.Fatalf("Close did not release port synchronously: %v", err)
	}
	_ = check.Close()
}

func TestConcurrentCloseAndEnableReleaseFixedPort(t *testing.T) {
	for i := 0; i < 10; i++ {
		cfg := DefaultConfig()
		cfg.UDPPort = unusedUDPPort(t)
		m, err := New(cfg, Hooks{Allowed: func(uint64, uint64) bool { return true }})
		if err != nil {
			t.Fatal(err)
		}
		enabled := cfg.Policy
		enabled.Enabled = true
		var wg sync.WaitGroup
		wg.Add(2)
		go func() { defer wg.Done(); _ = m.SetPolicy(enabled) }()
		go func() { defer wg.Done(); m.Close() }()
		wg.Wait()
		c, err := net.ListenUDP("udp4", &net.UDPAddr{Port: int(cfg.UDPPort)})
		if err != nil {
			t.Fatalf("concurrent shutdown leaked port: %v", err)
		}
		_ = c.Close()
	}
}

// Two publishers and three viewers negotiate independent WebRTC/DTLS sessions.
// Their server candidates must all use the same fixed port, while decrypted RTP
// payloads stay in their authorized room even after one viewer/room is closed.
func TestSinglePortConcurrentRoomsMediaIsolationAndPeerCleanup(t *testing.T) {
	m, move := testManager(t)
	move(4, 8)
	move(5, 7)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	settings := Settings{Width: 640, Height: 360, FPS: 30, BitrateKbps: 1000}
	type source struct {
		state  State
		track  *webrtc.TrackLocalStaticRTP
		seq    uint16
		marker byte
	}
	publish := func(session, room uint64, marker byte) *source {
		pc := testPeer(t)
		track, err := webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000}, "screen-video", "screen")
		if err != nil {
			t.Fatal(err)
		}
		sender, err := pc.AddTrack(track)
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
		state, answer, err := m.Publish(ctx, session, room, settings, offer(t, pc))
		if err != nil {
			t.Fatal(err)
		}
		assertCandidatePort(t, answer, m.config.UDPPort)
		if err = pc.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeAnswer, SDP: answer}); err != nil {
			t.Fatal(err)
		}
		wait(t, func() bool { return pc.ConnectionState() == webrtc.PeerConnectionStateConnected })
		return &source{state: state, track: track, marker: marker}
	}
	a, b := publish(1, 7, 0xA1), publish(3, 8, 0xB2)
	send := func(s *source) {
		s.seq++
		if err := s.track.WriteRTP(&rtp.Packet{Header: rtp.Header{Version: 2, SequenceNumber: s.seq, Timestamp: uint32(s.seq) * 9000, Marker: true}, Payload: []byte{0x10, 1, s.marker}}); err != nil {
			t.Fatal(err)
		}
	}
	send(a)
	send(b)
	wait(t, func() bool { return m.Status(7).Ready && m.Status(8).Ready })
	type packetTag struct {
		marker   byte
		sequence uint16
	}
	type sink struct {
		pc      *webrtc.PeerConnection
		packets chan packetTag
	}
	subscribe := func(session, room uint64, id string) *sink {
		pc := testPeer(t)
		ch := make(chan packetTag, 128)
		_, err := pc.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo, webrtc.RTPTransceiverInit{Direction: webrtc.RTPTransceiverDirectionRecvonly})
		if err != nil {
			t.Fatal(err)
		}
		pc.OnTrack(func(track *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
			for {
				p, _, err := track.ReadRTP()
				if err != nil {
					return
				}
				if len(p.Payload) >= 3 {
					select {
					case ch <- packetTag{marker: p.Payload[2], sequence: p.SequenceNumber}:
					default:
					}
				}
			}
		})
		_, answer, err := m.Subscribe(ctx, session, room, id, offer(t, pc))
		if err != nil {
			t.Fatal(err)
		}
		assertCandidatePort(t, answer, m.config.UDPPort)
		if err = pc.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeAnswer, SDP: answer}); err != nil {
			t.Fatal(err)
		}
		wait(t, func() bool { return pc.ConnectionState() == webrtc.PeerConnectionStateConnected })
		return &sink{pc: pc, packets: ch}
	}
	av, extra, bv := subscribe(2, 7, a.state.ShareID), subscribe(5, 7, a.state.ShareID), subscribe(4, 8, b.state.ShareID)
	if _, _, err := m.Subscribe(ctx, 4, 7, a.state.ShareID, "unused"); err == nil {
		t.Fatal("foreign-room member subscribed")
	}
	if _, _, err := m.Subscribe(ctx, 2, 7, b.state.ShareID, "unused"); err == nil {
		t.Fatal("foreign-room share ID subscribed")
	}
	expect := func(s *source, v *sink) {
		// Require a fresh sequence so queued pre-teardown media cannot make
		// the checks after closing a sibling viewer/room pass accidentally.
		minimum := s.seq + 1
		send(s)
		deadline := time.NewTimer(5 * time.Second)
		defer deadline.Stop()
		tick := time.NewTicker(50 * time.Millisecond)
		defer tick.Stop()
		for {
			select {
			case got := <-v.packets:
				if got.marker != s.marker {
					t.Fatalf("cross-room RTP: got %x want %x", got.marker, s.marker)
				}
				if got.sequence >= minimum {
					return
				}
			case <-tick.C:
				send(s)
			case <-deadline.C:
				t.Fatal("RTP did not arrive")
			}
		}
	}
	for i := 0; i < 5; i++ {
		send(a)
		send(b)
		expect(a, av)
		expect(a, extra)
		expect(b, bv)
	}
	m.Leave(5, 7, a.state.ShareID)
	_ = extra.pc.Close()
	if m.Status(7).Viewers != 1 {
		t.Fatal("individual viewer cleanup affected sibling")
	}
	for len(av.packets) > 0 {
		<-av.packets
	}
	for len(bv.packets) > 0 {
		<-bv.packets
	}
	expect(a, av)
	expect(b, bv)
	if err := m.Stop(1, 7, a.state.ShareID, false); err != nil {
		t.Fatal(err)
	}
	// Drain earlier packets so the next assertion proves forwarding after teardown.
	for len(bv.packets) > 0 {
		<-bv.packets
	}
	expect(b, bv)
	if m.Status(8).ShareID != b.state.ShareID {
		t.Fatal("closing one room affected other room")
	}
	m.Close()
	c, err := net.ListenUDP("udp4", &net.UDPAddr{Port: int(m.config.UDPPort)})
	if err != nil {
		t.Fatalf("active connections retained server port after Close: %v", err)
	}
	_ = c.Close()
}

// Public candidates need not be an address assigned to a local interface. The
// shared wildcard socket must still receive STUN and send replies correctly.
func TestFixedPortMuxPublicMappingIPv4AndIPv6(t *testing.T) {
	for _, tt := range []struct{ name, network, advertised, destination string }{
		{"IPv4 NAT", "udp4", "203.0.113.20", "127.0.0.1"},
		{"IPv6", "udp6", "::1", "::1"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			cfg := DefaultConfig()
			cfg.UDPPort = unusedUDPPort(t)
			cfg.PublicIP = tt.advertised
			mux, err := listenFixedUDPMux(cfg)
			if err != nil {
				if tt.network == "udp6" && ipv6Unavailable(err) {
					t.Skip("OS has disabled IPv6")
				}
				t.Fatal(err)
			}
			defer mux.Close()
			addresses := mux.GetListenAddresses()
			if len(addresses) != 1 || addresses[0].(*net.UDPAddr).IP.String() != tt.advertised || addresses[0].(*net.UDPAddr).Port != int(cfg.UDPPort) {
				t.Fatalf("incorrect public candidate: %v", addresses)
			}
			virtual, err := mux.GetConn("fixed-port-test", addresses[0])
			if err != nil {
				t.Fatal(err)
			}
			defer virtual.Close()
			client, err := net.ListenUDP(tt.network, &net.UDPAddr{IP: net.ParseIP(tt.destination)})
			if err != nil {
				t.Fatal(err)
			}
			defer client.Close()
			packet := stun.MustBuild(stun.TransactionID, stun.BindingRequest, stun.NewUsername("fixed-port-test:client"))
			if _, err = client.WriteToUDP(packet.Raw, &net.UDPAddr{IP: net.ParseIP(tt.destination), Port: int(cfg.UDPPort)}); err != nil {
				t.Fatal(err)
			}
			received := make(chan error, 1)
			go func() {
				buf := make([]byte, 2048)
				n, addr, err := virtual.ReadFrom(buf)
				if err == nil && !bytes.Equal(buf[:n], packet.Raw) {
					err = errors.New("mapped STUN payload changed")
				}
				if err == nil {
					_, err = virtual.WriteTo([]byte("reply"), addr)
				}
				received <- err
			}()
			select {
			case err = <-received:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("public mapping discarded inbound wildcard packet")
			}
			_ = client.SetReadDeadline(time.Now().Add(3 * time.Second))
			buf := make([]byte, 32)
			n, _, err := client.ReadFromUDP(buf)
			if err != nil || string(buf[:n]) != "reply" {
				t.Fatalf("mapped reply failed: %q %v", buf[:n], err)
			}
		})
	}
}
