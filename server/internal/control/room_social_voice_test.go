package control

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"image"
	"image/jpeg"
	"io"
	"log/slog"
	"net"
	"path/filepath"
	"sort"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
)

type socialVoicePeer struct {
	control  *testClient
	events   chan protocol.Envelope
	outbound *security.MediaCipher
	inbound  *security.MediaCipher
	udp      *net.UDPConn
}

// Derive independent client media keys from the real TLS identity handshake.
// Unlike a relay-only fixture, the five UDP senders are the same authenticated
// people who upload/download the image and send the room reactions.
func newSocialVoicePeer(t *testing.T, server *Server, tlsConfig *tls.Config, index int) *socialVoicePeer {
	t.Helper()
	public, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	ephemeral, err := security.NewEphemeralKey()
	if err != nil {
		t.Fatal(err)
	}
	defer ephemeral.Destroy()
	serverConn, clientConn := net.Pipe()
	go server.handle(tls.Server(serverConn, tlsConfig))
	conn := tls.Client(clientConn, &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13})
	t.Cleanup(func() { _ = conn.Close() })
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	c := &testClient{conn: conn, enc: json.NewEncoder(conn), dec: json.NewDecoder(conn), clientID: security.ClientID(public)}
	c.send(t, protocol.TypeHello, protocol.Hello{Nickname: fmt.Sprintf("voice-social-%d", index), ProtocolVersion: security.ProtocolVersion, SecurityCapabilities: requiredSecurityCapabilities(), ClientID: c.clientID, ClientPublicKey: base64.RawStdEncoding.EncodeToString(public), ClientEphemeralKey: base64.RawStdEncoding.EncodeToString(ephemeral.PublicKey())})
	var challenge protocol.IdentityChallenge
	if err := c.receiveUntil(t, protocol.TypeIdentityChallenge).Decode(&challenge); err != nil {
		t.Fatal(err)
	}
	decode := func(value string) []byte {
		b, err := base64.RawStdEncoding.DecodeString(value)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	serverEphemeral := decode(challenge.ServerEphemeralKey)
	transcript := security.AuthTranscript(decode(challenge.Challenge), decode(challenge.ServerPublicKey), ephemeral.PublicKey(), serverEphemeral)
	secrets, err := security.DeriveSessionSecrets(ephemeral, serverEphemeral, transcript)
	if err != nil {
		t.Fatal(err)
	}
	defer secrets.Destroy()
	outbound, err := security.NewMediaCipher(secrets.ClientToServerKey, secrets.ClientNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		t.Fatal(err)
	}
	inbound, err := security.NewMediaCipher(secrets.ServerToClientKey, secrets.ServerNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		t.Fatal(err)
	}
	c.send(t, protocol.TypeIdentityResponse, protocol.IdentityResponse{Signature: base64.RawStdEncoding.EncodeToString(ed25519.Sign(private, transcript))})
	var welcome protocol.Welcome
	if err := c.receiveUntil(t, protocol.TypeWelcome).Decode(&welcome); err != nil {
		t.Fatal(err)
	}
	c.sessionID = welcome.SessionID
	c.receiveUntil(t, protocol.TypeServerState)
	c.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 1})
	c.receiveUntil(t, protocol.TypeChatHistory)
	_ = c.conn.SetReadDeadline(time.Now().Add(15 * time.Second))
	peer := &socialVoicePeer{control: c, events: make(chan protocol.Envelope, 128), outbound: outbound, inbound: inbound}
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer close(peer.events)
		for {
			var envelope protocol.Envelope
			if err := c.dec.Decode(&envelope); err != nil {
				return
			}
			select {
			case peer.events <- envelope:
			default:
				return
			}
		}
	}()
	t.Cleanup(func() { _ = conn.Close(); <-done; outbound.Destroy(); inbound.Destroy() })
	return peer
}

func TestRoomSocialTransfersPreserveFivePersonUDPVoice(t *testing.T) {
	server, _ := socialServer(t)
	identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "identity.json"))
	if err != nil {
		t.Fatal(err)
	}
	tlsConfig, err := identity.TLSConfig("voice-social-test")
	if err != nil {
		t.Fatal(err)
	}
	server.identity, server.tlsConfig = identity, tlsConfig
	relaySocket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	relay := voice.New("", server.state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	relayDone := make(chan struct{})
	go func() { defer close(relayDone); _ = relay.Serve(relaySocket) }()
	t.Cleanup(func() { _ = relaySocket.Close(); <-relayDone })
	address := relaySocket.LocalAddr().(*net.UDPAddr)
	peers := make([]*socialVoicePeer, 5)
	knownIDs := map[uint64]bool{}
	for i := range peers {
		peers[i] = newSocialVoicePeer(t, server, tlsConfig, i)
		knownIDs[peers[i].control.sessionID] = true
		peers[i].udp, err = net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = peers[i].udp.Close() })
		packet, err := peers[i].outbound.SealPacket(protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: peers[i].control.sessionID}, nil)
		if err != nil {
			t.Fatal(err)
		}
		encoded, err := packet.MarshalBinary()
		if err != nil {
			t.Fatal(err)
		}
		if _, err = peers[i].udp.WriteToUDP(encoded, address); err != nil {
			t.Fatal(err)
		}
	}
	until := time.Now().Add(2 * time.Second)
	for _, peer := range peers {
		for server.state.CheckUDPSource(peer.control.sessionID, peer.udp.LocalAddr().(*net.UDPAddr)) != serverstate.UDPSourceValid {
			if time.Now().After(until) {
				t.Fatal("authenticated UDP registration timed out")
			}
			time.Sleep(time.Millisecond)
		}
	}
	var sent, received, invalid atomic.Uint64
	var readers sync.WaitGroup
	delays := make(chan []time.Duration, len(peers))
	for _, peer := range peers {
		readers.Add(1)
		go func(peer *socialVoicePeer) {
			defer readers.Done()
			buffer := make([]byte, protocol.MaxVoicePacketSize)
			previous := map[uint64]uint32{}
			var localDelays []time.Duration
			defer func() { delays <- localDelays }()
			for {
				n, _, err := peer.udp.ReadFromUDP(buffer)
				if err != nil {
					return
				}
				packet, err := protocol.ParseVoicePacket(buffer[:n])
				if err != nil || packet.Type != protocol.VoiceTypeAudio || !knownIDs[packet.SessionID] || packet.SessionID == peer.control.sessionID {
					invalid.Add(1)
					continue
				}
				payload, err := peer.inbound.Open(buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload)
				if err != nil || len(payload) != 120 {
					invalid.Add(1)
					continue
				}
				if packet.Sequence != previous[packet.SessionID]+1 || binary.BigEndian.Uint64(payload[8:16]) != packet.SessionID || binary.BigEndian.Uint32(payload[16:20]) != packet.Sequence {
					invalid.Add(1)
				}
				previous[packet.SessionID] = packet.Sequence
				for _, value := range payload[20:] {
					if value != byte(packet.Sequence) {
						invalid.Add(1)
						break
					}
				}
				localDelays = append(localDelays, time.Duration(max(0, time.Now().UnixNano()-int64(binary.BigEndian.Uint64(payload[:8])))))
				received.Add(1)
			}
		}(peer)
	}
	stopVoice, voiceDone := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(voiceDone)
		ticker := time.NewTicker(20 * time.Millisecond)
		defer ticker.Stop()
		var sequence uint32
		for {
			select {
			case <-stopVoice:
				return
			case <-ticker.C:
				sequence++
				for _, peer := range peers {
					payload := bytes.Repeat([]byte{byte(sequence)}, 120)
					binary.BigEndian.PutUint64(payload[:8], uint64(time.Now().UnixNano()))
					binary.BigEndian.PutUint64(payload[8:16], peer.control.sessionID)
					binary.BigEndian.PutUint32(payload[16:20], sequence)
					packet, err := peer.outbound.SealPacket(protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: peer.control.sessionID, Sequence: sequence}, payload)
					if err != nil {
						invalid.Add(1)
						continue
					}
					encoded, err := packet.MarshalBinary()
					if err != nil {
						invalid.Add(1)
						continue
					}
					if _, err = peer.udp.WriteToUDP(encoded, address); err != nil {
						invalid.Add(1)
					} else {
						sent.Add(1)
					}
				}
			}
		}
	}()
	var stopOnce sync.Once
	stop := func() { stopOnce.Do(func() { close(stopVoice); <-voiceDone }) }
	t.Cleanup(func() {
		stop()
		for _, peer := range peers {
			_ = peer.udp.Close()
		}
		readers.Wait()
	})

	var requestSequence, busyRetries int
	request := func(peer *socialVoicePeer, req protocol.RoomSocialRequest) protocol.RoomSocialEvent {
		t.Helper()
		deadline := time.Now().Add(10 * time.Second)
		for {
			requestSequence++
			req.RequestID, req.ChannelID = fmt.Sprintf("voice-social-%d", requestSequence), 1
			peer.control.send(t, protocol.TypeRoomSocialRequest, req)
			var event protocol.RoomSocialEvent
			for {
				select {
				case envelope, ok := <-peer.events:
					if !ok {
						t.Fatal("TLS control connection closed during voice")
					}
					if envelope.Type != protocol.TypeRoomSocialEvent {
						continue
					}
					if err := envelope.Decode(&event); err != nil {
						t.Fatal(err)
					}
					if event.RequestID != req.RequestID {
						continue
					}
				case <-time.After(time.Until(deadline)):
					t.Fatal("social response timed out while voice was active")
				}
				break
			}
			if event.Kind != "error" {
				return event
			}
			if event.Code != "busy" || event.RetryAfterMS <= 0 {
				t.Fatalf("social operation failed: %+v", event)
			}
			wait := time.Duration(event.RetryAfterMS) * time.Millisecond
			if time.Now().Add(wait).After(deadline) {
				t.Fatal("production rate limit did not recover")
			}
			busyRetries++
			time.Sleep(wait)
		}
	}
	// Deterministic textured JPEG crosses several 8 KiB chunks and exhausts the
	// production wire budget. Do not reset tokens or relax any production limits.
	const width, height = 256, 192
	picture := image.NewRGBA(image.Rect(0, 0, width, height))
	var random uint32 = 7
	for offset := 0; offset < len(picture.Pix); offset += 4 {
		for color := 0; color < 3; color++ {
			random = random*1664525 + 1013904223
			picture.Pix[offset+color] = byte(random >> 24)
		}
		picture.Pix[offset+3] = 255
	}
	var encodedImage bytes.Buffer
	if err := jpeg.Encode(&encodedImage, picture, &jpeg.Options{Quality: 85}); err != nil {
		t.Fatal(err)
	}
	imageData := encodedImage.Bytes()
	if len(imageData) < 3*protocol.MaxRoomImageChunkBytes || len(imageData) > protocol.MaxRoomImageBytes {
		t.Fatalf("unexpected fixture image size: %d", len(imageData))
	}
	begin := request(peers[0], protocol.RoomSocialRequest{Action: "image_begin", Width: width, Height: height, Bytes: len(imageData)})
	if begin.Kind != "image_upload" || begin.UploadID == "" {
		t.Fatalf("begin: %+v", begin)
	}
	for offset, chunk := 0, 0; offset < len(imageData); chunk++ {
		end := min(offset+protocol.MaxRoomImageChunkBytes, len(imageData))
		event := request(peers[0], protocol.RoomSocialRequest{Action: "image_chunk", UploadID: begin.UploadID, Offset: offset, Data: base64.StdEncoding.EncodeToString(imageData[offset:end])})
		if event.Kind != "image_upload" || event.Offset != end {
			t.Fatalf("chunk acknowledgment: %+v", event)
		}
		if chunk < len(peers) {
			reaction := request(peers[chunk], protocol.RoomSocialRequest{Action: "reaction", Emoji: "👍"})
			if reaction.Kind != "reaction" || reaction.SessionID != peers[chunk].control.sessionID {
				t.Fatalf("reaction: %+v", reaction)
			}
		}
		offset = end
	}
	committed := request(peers[0], protocol.RoomSocialRequest{Action: "image_commit", UploadID: begin.UploadID})
	if committed.Kind != "image_committed" || committed.Image == nil || committed.Image.Bytes != len(imageData) {
		t.Fatalf("commit: %+v", committed)
	}
	var downloaded []byte
	for len(downloaded) < len(imageData) {
		event := request(peers[1], protocol.RoomSocialRequest{Action: "image_get", Reference: committed.Image.Reference, Offset: len(downloaded)})
		if event.Kind != "image_data" || event.Reference != committed.Image.Reference || event.Offset != len(downloaded) {
			t.Fatalf("download: %+v", event)
		}
		part, err := base64.StdEncoding.DecodeString(event.Data)
		if err != nil || len(part) == 0 || len(part) > protocol.MaxRoomImageChunkBytes {
			t.Fatal("invalid image chunk")
		}
		downloaded = append(downloaded, part...)
		if event.EOF != (len(downloaded) == len(imageData)) {
			t.Fatal("incorrect image EOF")
		}
	}
	if !bytes.Equal(downloaded, imageData) {
		t.Fatal("image content changed while voice was relayed")
	}
	if busyRetries == 0 {
		t.Fatal("fixture did not exercise production busy/retry behavior")
	}
	stop()
	deadline := time.Now().Add(2 * time.Second)
	for received.Load() < sent.Load()*4 && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	for _, peer := range peers {
		_ = peer.udp.Close()
	}
	readers.Wait()
	close(delays)
	var latency []time.Duration
	for sample := range delays {
		latency = append(latency, sample...)
	}
	sort.Slice(latency, func(i, j int) bool { return latency[i] < latency[j] })
	if len(latency) > 0 {
		t.Logf("loopback encrypted UDP voice: sent=%d forwarded=%d/%d invalid=%d local p95=%s max=%s; JPEG=%d bytes, busy retries=%d (transport continuity only, not public-network or listening quality)", sent.Load(), received.Load(), sent.Load()*4, invalid.Load(), latency[(len(latency)-1)*95/100], latency[len(latency)-1], len(imageData), busyRetries)
	}
	if sent.Load() < 100 || received.Load() != sent.Load()*4 || invalid.Load() != 0 {
		t.Fatalf("voice discontinuity: sent=%d forwarded=%d expected=%d invalid=%d", sent.Load(), received.Load(), sent.Load()*4, invalid.Load())
	}
	metrics := relay.Metrics()
	if metrics.AuthFailures != 0 || metrics.WorkerQueueDrops != 0 || metrics.WriterQueueDrops != 0 || metrics.StaleQueueDrops != 0 || metrics.WriterStaleDrops != 0 {
		t.Fatalf("unexpected voice relay drops: %+v", metrics)
	}
}
