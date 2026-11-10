// Command voice-smoke verifies the production v0.5 control and voice paths.
package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/jpeg"
	"net"
	"os"
	"strings"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
)

var requiredCapabilities = []string{
	"identity_ed25519",
	"session_x25519_hkdf_sha256",
	"media_chacha20poly1305",
	"media_replay_window",
}

type options struct {
	controlAddr string
	voiceAddr   string
	fingerprint string
	identity    string
	channelID   uint64
	timeout     time.Duration
	avatarCheck bool
	watchCheck  bool
	gameCheck   bool
}

type smokeClient struct {
	conn       *tls.Conn
	scanner    *bufio.Scanner
	encoder    *json.Encoder
	welcome    protocol.Welcome
	inbound    *security.MediaCipher
	outbound   *security.MediaCipher
	channelID  uint64
	udp        *net.UDPConn
	udpAddress string
}

type smokeResult struct {
	TLSVersion        string `json:"tls_version"`
	ServerFingerprint string `json:"server_fingerprint"`
	ClientID          string `json:"client_id"`
	FirstSessionID    uint64 `json:"first_session_id"`
	SecondSessionID   uint64 `json:"second_session_id"`
	ChannelID         uint64 `json:"channel_id"`
	FirstUDPAddress   string `json:"first_udp_address"`
	SecondUDPAddress  string `json:"second_udp_address"`
	MediaRelay        string `json:"media_relay"`
	Status            string `json:"status"`
	AvatarCache       string `json:"avatar_cache,omitempty"`
	WatchParty        string `json:"watch_party,omitempty"`
}

func main() {
	var opts options
	flag.StringVar(&opts.controlAddr, "control", "", "TLS control address, for example 127.0.0.1:7001")
	flag.StringVar(&opts.voiceAddr, "voice", "", "UDP voice address, for example 127.0.0.1:7002")
	flag.StringVar(&opts.fingerprint, "fingerprint", "", "expected EdenVoice server identity fingerprint")
	flag.StringVar(&opts.identity, "identity", "", "persistent smoke identity path (recommended)")
	flag.Uint64Var(&opts.channelID, "channel", 1, "channel used for the two-session UDP discovery check")
	flag.DurationVar(&opts.timeout, "timeout", 8*time.Second, "timeout for each network phase")
	flag.BoolVar(&opts.avatarCheck, "avatar-cache-check", false, "upload and fetch one bounded custom avatar")
	flag.BoolVar(&opts.watchCheck, "watch-check", false, "verify two-viewer watch synchronization")
	flag.BoolVar(&opts.gameCheck, "game-check", false, "verify game catalog and activity protocol")
	flag.Parse()

	if err := run(opts); err != nil {
		fmt.Fprintln(os.Stderr, "voice-smoke:", err)
		os.Exit(1)
	}
}

func run(opts options) error {
	if opts.controlAddr == "" || opts.voiceAddr == "" || opts.fingerprint == "" {
		return errors.New("-control, -voice, and -fingerprint are required")
	}
	if opts.channelID == 0 {
		return errors.New("-channel must identify a joinable channel")
	}
	if opts.timeout <= 0 {
		return errors.New("-timeout must be positive")
	}

	identity, err := loadIdentity(opts.identity)
	if err != nil {
		return err
	}
	first, err := connect(opts, identity, "EdenVoice smoke A")
	if err != nil {
		return fmt.Errorf("first session: %w", err)
	}
	defer first.close()
	second, err := connect(opts, identity, "EdenVoice smoke B")
	if err != nil {
		return fmt.Errorf("second session: %w", err)
	}
	defer second.close()

	if err := first.join(opts.channelID, opts.timeout); err != nil {
		return fmt.Errorf("first session join: %w", err)
	}
	if err := second.join(opts.channelID, opts.timeout); err != nil {
		return fmt.Errorf("second session join: %w", err)
	}
	if opts.gameCheck {
		if err := verifyGames(first, second, opts.timeout); err != nil {
			return fmt.Errorf("game catalog: %w", err)
		}
	}
	avatarCache := ""
	watchResult := ""
	if opts.watchCheck {
		if err := verifyWatchPair(first, second, opts.timeout); err != nil {
			return fmt.Errorf("watch party: %w", err)
		}
		watchResult = "feature, join, readiness, seek, play, pause and leave verified; room lifecycle checked when advertised"
	}
	if opts.avatarCheck {
		if err := first.verifyAvatarCache(opts.timeout); err != nil {
			return fmt.Errorf("custom avatar cache protocol: %w", err)
		}
		avatarCache = "content hash reference and on-demand fetch verified"
	}
	if err := first.registerUDP(opts.voiceAddr, opts.timeout); err != nil {
		return fmt.Errorf("first UDP registration: %w", err)
	}
	firstAddress, err := second.waitForReflexiveCandidate(first.welcome.SessionID, opts.timeout)
	if err != nil {
		return fmt.Errorf("observe first UDP registration: %w", err)
	}
	if err := second.registerUDP(opts.voiceAddr, opts.timeout); err != nil {
		return fmt.Errorf("second UDP registration: %w", err)
	}
	secondAddress, err := first.waitForReflexiveCandidate(second.welcome.SessionID, opts.timeout)
	if err != nil {
		return fmt.Errorf("observe second UDP registration: %w", err)
	}
	firstPayload := []byte("EdenVoice/v0.5/smoke/A-to-B")
	if err := first.sendMedia(firstPayload, opts.timeout); err != nil {
		return fmt.Errorf("send first media packet: %w", err)
	}
	if err := second.receiveMedia(first.welcome.SessionID, firstPayload, opts.timeout); err != nil {
		return fmt.Errorf("receive first media packet: %w", err)
	}
	secondPayload := []byte("EdenVoice/v0.5/smoke/B-to-A")
	if err := second.sendMedia(secondPayload, opts.timeout); err != nil {
		return fmt.Errorf("send second media packet: %w", err)
	}
	if err := first.receiveMedia(second.welcome.SessionID, secondPayload, opts.timeout); err != nil {
		return fmt.Errorf("receive second media packet: %w", err)
	}

	result := smokeResult{
		TLSVersion:        "TLS 1.3",
		ServerFingerprint: first.welcome.ServerFingerprint,
		ClientID:          first.welcome.ClientID,
		FirstSessionID:    first.welcome.SessionID,
		SecondSessionID:   second.welcome.SessionID,
		ChannelID:         opts.channelID,
		FirstUDPAddress:   firstAddress,
		SecondUDPAddress:  secondAddress,
		MediaRelay:        "bidirectional encrypted payloads verified",
		Status:            "ok",
		AvatarCache:       avatarCache,
		WatchParty:        watchResult,
	}
	encoded, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		return err
	}
	fmt.Println(string(encoded))
	return nil
}

func (c *smokeClient) verifyAvatarCache(timeout time.Duration) error {
	avatar := image.NewRGBA(image.Rect(0, 0, protocol.AvatarSide, protocol.AvatarSide))
	for y := 0; y < protocol.AvatarSide; y++ {
		for x := 0; x < protocol.AvatarSide; x++ {
			avatar.SetRGBA(x, y, color.RGBA{R: uint8(x), G: uint8(y), B: 150, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := jpeg.Encode(&encoded, avatar, &jpeg.Options{Quality: 70}); err != nil {
		return err
	}
	upload := protocol.CustomAvatarPrefix + base64.StdEncoding.EncodeToString(encoded.Bytes())
	if err := c.send(protocol.TypeSetAvatar, protocol.AvatarRequest{Avatar: upload}); err != nil {
		return err
	}
	deadline := time.Now().Add(timeout)
	reference := ""
	for time.Now().Before(deadline) && reference == "" {
		envelope, err := c.read(time.Until(deadline))
		if err != nil {
			return err
		}
		if envelope.Type != protocol.TypeServerState {
			continue
		}
		var state protocol.ServerState
		if err := envelope.Decode(&state); err != nil {
			return err
		}
		for _, user := range state.Users {
			if user.SessionID == c.welcome.SessionID && strings.HasPrefix(user.Avatar, protocol.CustomAvatarRefPrefix) {
				reference = user.Avatar
			}
		}
	}
	if reference == "" {
		return errors.New("server snapshot did not publish a custom avatar reference")
	}
	if err := c.send(protocol.TypeGetAvatar, protocol.AvatarRequest{Avatar: reference}); err != nil {
		return err
	}
	deadline = time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		envelope, err := c.read(time.Until(deadline))
		if err != nil {
			return err
		}
		if envelope.Type != protocol.TypeAvatarData {
			continue
		}
		var response protocol.AvatarData
		if err := envelope.Decode(&response); err != nil {
			return err
		}
		if response.Avatar != reference || response.Data != upload {
			return errors.New("downloaded avatar did not match the uploaded content")
		}
		return nil
	}
	return errors.New("timed out waiting for avatar data")
}

func loadIdentity(path string) (*security.Identity, error) {
	if path != "" {
		identity, _, err := security.LoadOrCreateIdentity(path)
		if err != nil {
			return nil, fmt.Errorf("load identity: %w", err)
		}
		return identity, nil
	}
	publicKey, privateKey, err := ed25519.GenerateKey(cryptorand.Reader)
	if err != nil {
		return nil, fmt.Errorf("generate ephemeral identity: %w", err)
	}
	return &security.Identity{PublicKey: publicKey, PrivateKey: privateKey}, nil
}

func connect(opts options, identity *security.Identity, nickname string) (*smokeClient, error) {
	dialer := &net.Dialer{Timeout: opts.timeout}
	// The deployment uses its Ed25519 identity as the trust anchor. Certificate
	// verification is therefore performed explicitly against -fingerprint below.
	conn, err := tls.DialWithDialer(dialer, "tcp", opts.controlAddr, &tls.Config{
		MinVersion:         tls.VersionTLS13,
		MaxVersion:         tls.VersionTLS13,
		InsecureSkipVerify: true, //nolint:gosec -- pinned EdenVoice identity is checked below.
	})
	if err != nil {
		return nil, fmt.Errorf("TLS dial: %w", err)
	}
	client := &smokeClient{conn: conn, scanner: bufio.NewScanner(conn), encoder: json.NewEncoder(conn), channelID: opts.channelID}
	client.scanner.Buffer(make([]byte, 4096), protocol.MaxControlMessageBytes)
	failed := true
	defer func() {
		if failed {
			client.close()
		}
	}()

	state := conn.ConnectionState()
	if state.Version != tls.VersionTLS13 || len(state.PeerCertificates) != 1 {
		return nil, errors.New("server did not negotiate TLS 1.3 with one identity certificate")
	}
	ephemeral, err := security.NewEphemeralKey()
	if err != nil {
		return nil, err
	}
	defer ephemeral.Destroy()
	ephemeralPublic := ephemeral.PublicKey()
	clientID := security.ClientID(identity.PublicKey)
	hello := protocol.Hello{
		Nickname:             nickname,
		ProtocolVersion:      security.ProtocolVersion,
		SecurityCapabilities: requiredCapabilities,
		ClientID:             clientID,
		ClientPublicKey:      base64.RawStdEncoding.EncodeToString(identity.PublicKey),
		ClientEphemeralKey:   base64.RawStdEncoding.EncodeToString(ephemeralPublic),
	}
	if err := client.send(protocol.TypeHello, hello); err != nil {
		return nil, err
	}
	envelope, err := client.read(opts.timeout)
	if err != nil {
		return nil, err
	}
	if envelope.Type != protocol.TypeIdentityChallenge {
		return nil, unexpected(envelope, protocol.TypeIdentityChallenge)
	}
	var challenge protocol.IdentityChallenge
	if err := envelope.Decode(&challenge); err != nil {
		return nil, fmt.Errorf("decode identity challenge: %w", err)
	}
	serverPublic, transcript, err := verifyChallenge(challenge, opts.fingerprint, state.PeerCertificates[0].PublicKey, ephemeralPublic)
	if err != nil {
		return nil, err
	}
	serverEphemeral, err := base64.RawStdEncoding.DecodeString(challenge.ServerEphemeralKey)
	if err != nil {
		return nil, errors.New("invalid server ephemeral key encoding")
	}
	secrets, err := security.DeriveSessionSecrets(ephemeral, serverEphemeral, transcript)
	if err != nil {
		return nil, err
	}
	defer secrets.Destroy()
	outbound, err := security.NewMediaCipher(secrets.ClientToServerKey, secrets.ClientNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		return nil, err
	}
	inbound, err := security.NewMediaCipher(secrets.ServerToClientKey, secrets.ServerNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		outbound.Destroy()
		return nil, err
	}
	response := protocol.IdentityResponse{Signature: base64.RawStdEncoding.EncodeToString(ed25519.Sign(identity.PrivateKey, transcript))}
	if err := client.send(protocol.TypeIdentityResponse, response); err != nil {
		return nil, err
	}
	envelope, err = client.read(opts.timeout)
	if err != nil {
		return nil, err
	}
	if envelope.Type != protocol.TypeWelcome {
		return nil, unexpected(envelope, protocol.TypeWelcome)
	}
	if err := envelope.Decode(&client.welcome); err != nil {
		return nil, fmt.Errorf("decode welcome: %w", err)
	}
	if client.welcome.ClientID != clientID || client.welcome.ServerFingerprint != opts.fingerprint || client.welcome.MediaKeyEpoch != security.InitialKeyEpoch {
		return nil, errors.New("welcome identity or media epoch does not match the authenticated session")
	}
	if !bytes.Equal(serverPublic, state.PeerCertificates[0].PublicKey.(ed25519.PublicKey)) {
		return nil, errors.New("TLS and challenge server identities differ")
	}
	envelope, err = client.read(opts.timeout)
	if err != nil {
		return nil, err
	}
	if envelope.Type != protocol.TypeServerState {
		return nil, unexpected(envelope, protocol.TypeServerState)
	}
	var snapshot protocol.ServerState
	if err := envelope.Decode(&snapshot); err != nil {
		return nil, fmt.Errorf("decode server state: %w", err)
	}
	found := false
	for _, channel := range snapshot.Channels {
		found = found || channel.ID == opts.channelID
	}
	if !found {
		return nil, fmt.Errorf("channel %d is absent from server state", opts.channelID)
	}
	client.outbound = outbound
	client.inbound = inbound
	failed = false
	return client, nil
}

func verifyChallenge(challenge protocol.IdentityChallenge, expectedFingerprint string, certificatePublic any, clientEphemeral []byte) ([]byte, []byte, error) {
	serverPublic, err := base64.RawStdEncoding.DecodeString(challenge.ServerPublicKey)
	if err != nil || len(serverPublic) != ed25519.PublicKeySize {
		return nil, nil, errors.New("invalid server public key")
	}
	certificateKey, ok := certificatePublic.(ed25519.PublicKey)
	if !ok || !bytes.Equal(certificateKey, serverPublic) {
		return nil, nil, errors.New("TLS certificate is not bound to the challenge identity")
	}
	actualFingerprint := (&security.Identity{PublicKey: ed25519.PublicKey(serverPublic)}).Fingerprint()
	if challenge.ServerFingerprint != expectedFingerprint || actualFingerprint != expectedFingerprint {
		return nil, nil, fmt.Errorf("server fingerprint mismatch: expected %s, received %s", expectedFingerprint, challenge.ServerFingerprint)
	}
	if !sameCapabilities(challenge.RequiredCapabilities, requiredCapabilities) {
		return nil, nil, errors.New("server required security capabilities differ from the v0.5 profile")
	}
	challengeBytes, err := base64.RawStdEncoding.DecodeString(challenge.Challenge)
	if err != nil || len(challengeBytes) != 32 {
		return nil, nil, errors.New("invalid identity challenge")
	}
	serverEphemeral, err := base64.RawStdEncoding.DecodeString(challenge.ServerEphemeralKey)
	if err != nil || len(serverEphemeral) != 32 {
		return nil, nil, errors.New("invalid server ephemeral key")
	}
	return serverPublic, security.AuthTranscript(challengeBytes, serverPublic, clientEphemeral, serverEphemeral), nil
}

func sameCapabilities(actual, expected []string) bool {
	if len(actual) != len(expected) {
		return false
	}
	available := make(map[string]bool, len(actual))
	for _, capability := range actual {
		available[capability] = true
	}
	for _, capability := range expected {
		if !available[capability] {
			return false
		}
	}
	return true
}

func (c *smokeClient) send(kind protocol.MessageType, value any) error {
	envelope, err := protocol.NewEnvelope(kind, value)
	if err != nil {
		return err
	}
	if err := c.encoder.Encode(envelope); err != nil {
		return fmt.Errorf("send %s: %w", kind, err)
	}
	return nil
}

func (c *smokeClient) read(timeout time.Duration) (protocol.Envelope, error) {
	if err := c.conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
		return protocol.Envelope{}, err
	}
	if !c.scanner.Scan() {
		if err := c.scanner.Err(); err != nil {
			return protocol.Envelope{}, fmt.Errorf("read control message: %w", err)
		}
		return protocol.Envelope{}, errors.New("control connection closed")
	}
	var envelope protocol.Envelope
	if err := json.Unmarshal(c.scanner.Bytes(), &envelope); err != nil {
		return protocol.Envelope{}, fmt.Errorf("decode control envelope: %w", err)
	}
	if envelope.Type == protocol.TypeError {
		var response protocol.Error
		_ = envelope.Decode(&response)
		return protocol.Envelope{}, fmt.Errorf("server error %s: %s", response.Code, response.Message)
	}
	return envelope, nil
}

func (c *smokeClient) join(channelID uint64, timeout time.Duration) error {
	if err := c.send(protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: channelID}); err != nil {
		return err
	}
	deadline := time.Now().Add(timeout)
	joined := false
	historyReceived := false
	for time.Now().Before(deadline) {
		envelope, err := c.read(time.Until(deadline))
		if err != nil {
			return err
		}
		switch envelope.Type {
		case protocol.TypeUserMoved:
			var moved protocol.UserMoved
			if err := envelope.Decode(&moved); err != nil {
				return err
			}
			joined = moved.SessionID == c.welcome.SessionID && moved.ChannelID == channelID
		case protocol.TypeChatHistory:
			var history protocol.ChatHistory
			if err := envelope.Decode(&history); err != nil {
				return fmt.Errorf("decode chat history: %w", err)
			}
			if history.ChannelID == channelID {
				if history.Messages == nil {
					return errors.New("chat history messages encoded as null instead of a sequence")
				}
				historyReceived = true
			}
		}
		if joined && historyReceived {
			return nil
		}
	}
	return errors.New("timed out waiting for channel join confirmation and chat history")
}

func (c *smokeClient) registerUDP(address string, timeout time.Duration) error {
	remote, err := net.ResolveUDPAddr("udp", address)
	if err != nil {
		return err
	}
	dialer := net.Dialer{Timeout: timeout}
	conn, err := dialer.DialContext(context.Background(), "udp", remote.String())
	if err != nil {
		return err
	}
	udp, ok := conn.(*net.UDPConn)
	if !ok {
		_ = conn.Close()
		return errors.New("voice connection is not UDP")
	}
	packet := protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: c.welcome.SessionID, Sequence: 1, KeyEpoch: security.InitialKeyEpoch}
	packet, err = c.outbound.SealPacket(packet, nil)
	if err != nil {
		_ = udp.Close()
		return err
	}
	encoded, err := packet.MarshalBinary()
	if err != nil {
		_ = udp.Close()
		return err
	}
	if err := udp.SetWriteDeadline(time.Now().Add(timeout)); err != nil {
		_ = udp.Close()
		return err
	}
	if _, err := udp.Write(encoded); err != nil {
		_ = udp.Close()
		return err
	}
	c.udp = udp
	return nil
}

func (c *smokeClient) sendMedia(plaintext []byte, timeout time.Duration) error {
	if c.udp == nil {
		return errors.New("UDP registration is required before media")
	}
	packet := protocol.VoicePacket{
		Type:      protocol.VoiceTypeAudio,
		SessionID: c.welcome.SessionID,
		Sequence:  2,
		Timestamp: 960,
		KeyEpoch:  security.InitialKeyEpoch,
	}
	packet, err := c.outbound.SealPacket(packet, plaintext)
	if err != nil {
		return err
	}
	encoded, err := packet.MarshalBinary()
	if err != nil {
		return err
	}
	if err := c.udp.SetWriteDeadline(time.Now().Add(timeout)); err != nil {
		return err
	}
	if _, err := c.udp.Write(encoded); err != nil {
		return err
	}
	return nil
}

func (c *smokeClient) receiveMedia(senderID uint64, expected []byte, timeout time.Duration) error {
	if c.udp == nil {
		return errors.New("UDP registration is required before media")
	}
	if err := c.udp.SetReadDeadline(time.Now().Add(timeout)); err != nil {
		return err
	}
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	for {
		n, err := c.udp.Read(buffer)
		if err != nil {
			return err
		}
		packet, err := protocol.ParseVoicePacket(buffer[:n])
		if err != nil || packet.Type != protocol.VoiceTypeAudio || packet.SessionID != senderID {
			continue
		}
		plaintext, err := c.inbound.Open(buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload)
		if err != nil {
			return fmt.Errorf("decrypt relayed media: %w", err)
		}
		if !bytes.Equal(plaintext, expected) {
			return errors.New("relayed media plaintext differs from the sent payload")
		}
		return nil
	}
}

func (c *smokeClient) waitForReflexiveCandidate(peerID uint64, timeout time.Duration) (string, error) {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		envelope, err := c.read(time.Until(deadline))
		if err != nil {
			return "", err
		}
		if envelope.Type != protocol.TypePeerCandidates {
			continue
		}
		var candidates protocol.PeerCandidates
		if err := envelope.Decode(&candidates); err != nil {
			return "", err
		}
		if candidates.PeerID != peerID {
			continue
		}
		for _, candidate := range candidates.Candidates {
			if candidate.Kind == "server_reflexive" && strings.TrimSpace(candidate.Address) != "" {
				return candidate.Address, nil
			}
		}
	}
	return "", fmt.Errorf("timed out waiting for peer %d server-reflexive candidate", peerID)
}

func (c *smokeClient) close() {
	if c.udp != nil {
		_ = c.udp.Close()
	}
	if c.conn != nil {
		_ = c.conn.Close()
	}
}

func unexpected(envelope protocol.Envelope, expected protocol.MessageType) error {
	return fmt.Errorf("expected %s, received %s", expected, envelope.Type)
}
