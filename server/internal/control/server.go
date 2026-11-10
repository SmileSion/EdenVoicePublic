package control

import (
	"bufio"
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log/slog"
	"net"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/announcements"
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/clientupdates"
	"edenvoice/server/internal/gamecatalog"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/watchparty"
)

type Server struct {
	chatQueryMu          sync.Mutex
	chatLastQuery        map[uint64]time.Time
	chatQuerySlots       chan struct{}
	social               *roomSocial
	announcements        *announcements.Store
	screenPolicyMu       sync.Mutex
	screenPolicyPath     string
	screenPolicyDocument screenshare.PolicyDocument
	screen               *screenshare.Manager
	screenMu             sync.Mutex
	screenMembers        map[uint64]uint64
	screenPending        map[uint64]screenOperation
	serverPassword       string
	passwordAttempts     passwordAttemptLimiter
	updates              *clientupdates.Store
	catalogMu            sync.Mutex
	catalog              *gamecatalog.Store
	addr                 string
	voiceAddr            string
	state                *serverstate.State
	logger               *slog.Logger
	routeMu              sync.Mutex
	lastRoute            map[uint64]protocol.RouteUpdate
	tlsConfig            *tls.Config
	identity             *security.Identity
	allowLegacyInsecure  bool
	administration       *administration.Store
	archive              *archive.Collector
	auditMu              sync.Mutex
	auditEvents          []AdminAudit
	watchMu              sync.Mutex
	watch                *watchparty.Manager
	watchMembers         map[uint64]uint64
	watchLastRequest     map[uint64]time.Time
}

func New(addr, voiceAddr string, state *serverstate.State, logger *slog.Logger) *Server {
	return &Server{chatLastQuery: make(map[uint64]time.Time), chatQuerySlots: make(chan struct{}, 4), social: newRoomSocial(), announcements: announcements.New(), screenMembers: make(map[uint64]uint64), screenPending: make(map[uint64]screenOperation), updates: clientupdates.New(), addr: addr, voiceAddr: voiceAddr, state: state, logger: logger, lastRoute: make(map[uint64]protocol.RouteUpdate), watch: watchparty.New(), watchMembers: make(map[uint64]uint64), watchLastRequest: make(map[uint64]time.Time)}
}

func NewSecure(addr, voiceAddr string, state *serverstate.State, logger *slog.Logger, identity *security.Identity, tlsConfig *tls.Config, allowLegacyInsecure bool, administrationStore *administration.Store, archiveCollector ...*archive.Collector) *Server {
	server := New(addr, voiceAddr, state, logger)
	server.identity = identity
	server.tlsConfig = tlsConfig
	server.allowLegacyInsecure = allowLegacyInsecure
	server.administration = administrationStore
	if len(archiveCollector) > 0 {
		server.archive = archiveCollector[0]
	}
	return server
}

func (s *Server) ListenAndServe() error {
	listener, err := net.Listen("tcp", s.addr)
	if err != nil {
		return err
	}
	defer listener.Close()
	if s.tlsConfig != nil {
		listener = tls.NewListener(listener, s.tlsConfig)
	}
	s.logger.Info("control server started", "addr", listener.Addr())
	go s.cleanupExpired()
	go func() {
		t := time.NewTicker(time.Second)
		defer t.Stop()
		for now := range t.C {
			s.cleanupRoomSocial(now)
			if s.state.ReconcileGameScenes(now) {
				s.broadcastSnapshot()
			}
		}
	}()
	go s.reconcileRoutes()
	for {
		conn, err := listener.Accept()
		if err != nil {
			return err
		}
		go s.handle(conn)
	}
}

func (s *Server) reconcileRoutes() {
	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()
	for range ticker.C {
		for _, roomID := range s.state.ActiveRooms() {
			s.broadcastRoute(roomID)
		}
	}
}

type connSender struct {
	conn      net.Conn
	queue     chan protocol.Envelope
	done      chan struct{}
	closeOnce sync.Once
	pending   atomic.Int64
}

const controlSendQueueSize = 64
const mediaRekeyGrace = 5 * time.Second

func newConnSender(conn net.Conn) *connSender {
	sender := &connSender{
		conn:  conn,
		queue: make(chan protocol.Envelope, controlSendQueueSize),
		done:  make(chan struct{}),
	}
	go sender.writeLoop()
	return sender
}

func (s *connSender) writeLoop() {
	encoder := json.NewEncoder(s.conn)
	defer func() {
		for {
			select {
			case <-s.queue:
				s.pending.Add(-1)
			default:
				return
			}
		}
	}()
	for {
		select {
		case message := <-s.queue:
			err := encoder.Encode(message)
			s.pending.Add(-1)
			if err != nil {
				s.abort()
				return
			}
		case <-s.done:
			return
		}
	}
}

func (s *connSender) Close() error {
	deadline := time.Now().Add(100 * time.Millisecond)
	for s.pending.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	return s.abort()
}

func (s *connSender) abort() error {
	var err error
	s.closeOnce.Do(func() {
		close(s.done)
		if tlsConn, ok := s.conn.(*tls.Conn); ok {
			// A forced kick/ban must not wait for a peer that stopped reading the
			// TLS close_notify record. Closing the underlying transport also
			// unblocks any concurrent control write immediately.
			err = tlsConn.NetConn().Close()
		} else {
			err = s.conn.Close()
		}
	})
	return err
}

func (s *connSender) Send(msg protocol.Envelope) error {
	select {
	case <-s.done:
		return net.ErrClosed
	default:
	}
	s.pending.Add(1)
	select {
	case s.queue <- msg:
		return nil
	case <-s.done:
		s.pending.Add(-1)
		return net.ErrClosed
	default:
		s.pending.Add(-1)
		_ = s.abort()
		return errors.New("control send queue is full")
	}
}

func (s *Server) handle(conn net.Conn) {
	defer conn.Close()
	// First-use trust confirmation is an explicit user interaction. Keep the
	// pre-auth connection bounded, but allow enough time to inspect and accept
	// the server fingerprint before the client sends its identity response.
	_ = conn.SetDeadline(time.Now().Add(90 * time.Second))
	scanner := bufio.NewScanner(conn)
	scanner.Buffer(make([]byte, 4096), protocol.MaxControlMessageBytes)
	sender := newConnSender(conn)
	defer sender.Close()
	if !scanner.Scan() {
		s.sendError(sender, "invalid_hello", "HELLO is required")
		return
	}
	line := scanner.Bytes()
	var envelope protocol.Envelope
	if json.Unmarshal(line, &envelope) != nil || envelope.Type != protocol.TypeHello {
		s.sendError(sender, "invalid_hello", "HELLO is required")
		return
	}
	var hello protocol.Hello
	if envelope.Decode(&hello) != nil {
		s.sendError(sender, "invalid_hello", "invalid HELLO data")
		return
	}
	hello.Nickname = strings.TrimSpace(hello.Nickname)
	if hello.Nickname == "" || len(hello.Nickname) > protocol.MaxNicknameBytes || !utf8.ValidString(hello.Nickname) {
		s.sendError(sender, "invalid_nickname", "nickname must be valid UTF-8 and 1-64 bytes")
		return
	}
	if s.serverPassword != "" {
		if _, ok := conn.(*tls.Conn); !ok || s.identity == nil || hello.ProtocolVersion != security.ProtocolVersion {
			s.sendError(sender, "secure_mode_required", "password access requires a secure client")
			return
		}
		if !s.passwordAttempts.allow(conn.RemoteAddr().String(), time.Now()) {
			s.sendError(sender, "server_password_rate_limited", "too many authentication attempts; retry later")
			return
		}
	}
	var clientID, serverRole, avatar string
	var inbound, outbound *security.MediaCipher
	if s.identity != nil {
		if hello.ProtocolVersion != security.ProtocolVersion {
			if !s.allowLegacyInsecure || hello.ProtocolVersion != 0 {
				s.sendError(sender, "secure_mode_required", "EdenVoice v0.5 secure mode is required")
				return
			}
		} else {
			var authErr error
			clientID, inbound, outbound, authErr = s.authenticate(scanner, sender, hello)
			if authErr != nil {
				if errors.Is(authErr, errServerPasswordRequired) {
					s.sendError(sender, "server_password_required", "server password is required")
					return
				}
				if errors.Is(authErr, errServerPasswordInvalid) {
					s.sendError(sender, "server_password_invalid", "server password is incorrect")
					return
				}
				s.logger.Warn("identity authentication failed", "client_id", hello.ClientID, "error", authErr)
				s.sendError(sender, "identity_authentication_failed", "identity authentication failed")
				return
			}
			s.logger.Info("identity authentication succeeded", "client_id", clientID)
			if s.administration != nil {
				user, err := s.administration.Admit(clientID, hello.ClientPublicKey, hello.Nickname, time.Now())
				if err != nil {
					if errors.Is(err, administration.ErrBanned) {
						s.sendError(sender, "banned", "this identity is banned")
					} else {
						s.sendError(sender, "identity_admission_failed", "identity admission failed")
					}
					return
				}
				serverRole = string(user.ServerRole)
				avatar = user.Avatar
			}
		}
	}
	session := s.state.AddAuthenticatedSession(hello.Nickname, clientID, serverRole, hello.ClientEphemeralKey, sender, inbound, outbound)
	s.state.SetClientBuild(session.ID, hello.ClientVersion, hello.ClientPlatform)
	for _, feature := range hello.ClientFeatures {
		if feature == "chat_query_v1" {
			s.state.EnableChatQueries(session.ID)
		}
	}
	if avatar != "" {
		_ = s.state.SetAvatar(session.ID, avatar)
	}
	defer s.disconnect(session.ID, false)
	s.logger.Info("session connected", "session_id", session.ID, "nickname", session.Nickname)
	user, _ := s.state.User(session.ID)
	event, _ := protocol.NewEnvelope(protocol.TypeUserJoined, protocol.UserEvent{User: user})
	// Membership transitions are ordered protocol state. Broadcasting this in a
	// goroutine allowed a following USER_MOVED to overtake USER_JOINED. The late
	// join event still carries channel_id=0 and could therefore move a user back
	// out of the room in every existing client's view. connSender.Send is already
	// non-blocking, so preserve the authoritative order here.
	s.state.Broadcast(event, session.ID)
	welcome := protocol.Welcome{SessionID: session.ID, ServerName: s.state.Snapshot().ServerName, VoiceAddr: s.voiceAddr, MediaQueueSize: s.state.NetworkConfig().MediaQueueSize, ClientID: clientID, ServerFeatures: s.serverFeatures()}
	if s.identity != nil && clientID != "" {
		welcome.ServerFingerprint = s.identity.Fingerprint()
		welcome.MediaKeyEpoch = security.InitialKeyEpoch
		welcome.ServerRole = serverRole
		welcome.Capabilities = s.capabilities(clientID, 0)
		welcome.NetworkPolicyChannelIDs = s.networkPolicyChannelIDs(clientID)
	}
	s.send(sender, protocol.TypeWelcome, welcome)
	s.send(sender, protocol.TypeServerState, s.snapshot())
	_ = conn.SetDeadline(time.Now().Add(30 * time.Second))
	for scanner.Scan() {
		line = scanner.Bytes()
		_ = conn.SetDeadline(time.Now().Add(30 * time.Second))
		if json.Unmarshal(line, &envelope) != nil {
			s.sendError(sender, "invalid_json", "invalid control message")
			continue
		}
		if envelope.Type == protocol.TypeRoomSocialRequest {
			if _, secure := conn.(*tls.Conn); !secure {
				var req protocol.RoomSocialRequest
				_ = envelope.Decode(&req)
				s.socialError(sender, req, "secure_mode_required", "room social requires authenticated TLS", 0)
				continue
			}
		}
		if err := s.dispatch(session.ID, envelope, sender); err != nil {
			if errors.Is(err, serverstate.ErrUnknownChannel) {
				s.sendError(sender, "unknown_channel", "channel does not exist")
			} else if errors.Is(err, serverstate.ErrChannelFull) {
				s.sendError(sender, "channel_full", "channel user limit reached")
			} else if errors.Is(err, administration.ErrForbidden) {
				s.sendError(sender, "permission_denied", "you do not have permission")
			} else {
				s.sendError(sender, "invalid_message", err.Error())
			}
		}
	}
}

func (s *Server) serverFeatures() []string {
	features := []string{"chat_query_v1", "room_social_v1", "game_activity_v1", "game_catalog_v1", "auto_room_scene_v1", "watch_party_v1", "watch_rooms_v1", "watch_moderation_v1"}
	if s.social.images != nil {
		features = append(features, "room_images_v1")
	}
	if s.screen != nil {
		features = append(features, "screen_share_v1")
	}
	if s.archive == nil {
		return features
	}
	return append(features, "eden_archive_v1")
}

func (s *Server) authenticate(scanner *bufio.Scanner, sender *connSender, hello protocol.Hello) (string, *security.MediaCipher, *security.MediaCipher, error) {
	if !hasRequiredSecurityCapabilities(hello.SecurityCapabilities) {
		return "", nil, nil, errors.New("required security capabilities are missing")
	}
	clientPublicBytes, err := base64.RawStdEncoding.DecodeString(hello.ClientPublicKey)
	if err != nil || len(clientPublicBytes) != ed25519.PublicKeySize {
		return "", nil, nil, errors.New("invalid client public key")
	}
	clientEphemeral, err := base64.RawStdEncoding.DecodeString(hello.ClientEphemeralKey)
	if err != nil || len(clientEphemeral) != 32 {
		return "", nil, nil, errors.New("invalid client ephemeral key")
	}
	challenge, err := security.NewChallenge()
	if err != nil {
		return "", nil, nil, err
	}
	serverEphemeral, err := security.NewEphemeralKey()
	if err != nil {
		return "", nil, nil, err
	}
	defer serverEphemeral.Destroy()
	serverEphemeralPublic := serverEphemeral.PublicKey()
	transcript := security.AuthTranscript(challenge, s.identity.PublicKey, clientEphemeral, serverEphemeralPublic)
	s.send(sender, protocol.TypeIdentityChallenge, protocol.IdentityChallenge{
		ServerPasswordRequired: s.serverPassword != "",
		Challenge:              base64.RawStdEncoding.EncodeToString(challenge),
		ServerPublicKey:        base64.RawStdEncoding.EncodeToString(s.identity.PublicKey),
		ServerFingerprint:      s.identity.Fingerprint(),
		ServerEphemeralKey:     base64.RawStdEncoding.EncodeToString(serverEphemeralPublic),
		RequiredCapabilities:   requiredSecurityCapabilities(),
	})
	if !scanner.Scan() {
		return "", nil, nil, errors.New("identity response is required")
	}
	var envelope protocol.Envelope
	if err := json.Unmarshal(scanner.Bytes(), &envelope); err != nil || envelope.Type != protocol.TypeIdentityResponse {
		return "", nil, nil, errors.New("invalid identity response")
	}
	var response protocol.IdentityResponse
	if err := envelope.Decode(&response); err != nil {
		return "", nil, nil, errors.New("invalid identity response")
	}
	signature, err := base64.RawStdEncoding.DecodeString(response.Signature)
	if err != nil {
		return "", nil, nil, errors.New("invalid identity signature encoding")
	}
	publicKey := ed25519.PublicKey(clientPublicBytes)
	if err := security.VerifyClient(publicKey, hello.ClientID, transcript, signature); err != nil {
		return "", nil, nil, err
	}
	if err := checkServerPassword(s.serverPassword, response.ServerPassword); err != nil {
		return "", nil, nil, err
	}
	secrets, err := security.DeriveSessionSecrets(serverEphemeral, clientEphemeral, transcript)
	if err != nil {
		return "", nil, nil, err
	}
	defer secrets.Destroy()
	inbound, err := security.NewMediaCipher(secrets.ClientToServerKey, secrets.ClientNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		return "", nil, nil, err
	}
	outbound, err := security.NewMediaCipher(secrets.ServerToClientKey, secrets.ServerNoncePrefix, security.InitialKeyEpoch)
	if err != nil {
		inbound.Destroy()
		return "", nil, nil, err
	}
	return hello.ClientID, inbound, outbound, nil
}

func requiredSecurityCapabilities() []string {
	return []string{"identity_ed25519", "session_x25519_hkdf_sha256", "media_chacha20poly1305", "media_replay_window"}
}

func hasRequiredSecurityCapabilities(provided []string) bool {
	available := make(map[string]bool, len(provided))
	for _, capability := range provided {
		available[capability] = true
	}
	for _, required := range requiredSecurityCapabilities() {
		if !available[required] {
			return false
		}
	}
	return true
}

func (s *Server) cleanupExpired() {
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()
	for now := range ticker.C {
		if s.archive != nil {
			snapshot := s.state.Snapshot()
			names := map[uint64]string{}
			for _, room := range snapshot.Channels {
				names[room.ID] = room.Name
			}
			people := []archive.GamePresence{}
			for _, user := range snapshot.Users {
				if game := user.GameActivity; game != nil && game.ExpiresAt > now.UnixMilli() {
					people = append(people, archive.GamePresence{RoomID: user.ChannelID, RoomName: names[user.ChannelID], MemberID: user.ClientID, GameID: game.GameID, Title: game.Title})
				}
			}
			s.archive.ObserveGames(people, now)
		}

		for _, id := range s.state.ExpireGameActivities(now) {
			s.broadcastState(id)
		}
		for _, id := range s.state.Expired(now.Add(-30 * time.Second)) {
			s.logger.Warn("heartbeat timeout", "session_id", id)
			s.disconnect(id, true)
		}
	}
}

func (s *Server) dispatch(id uint64, envelope protocol.Envelope, sender serverstate.Sender) error {
	actorClientID := s.state.ClientID(id)
	switch envelope.Type {
	case protocol.TypeRoomSocialRequest:
		return s.dispatchRoomSocial(id, envelope, sender)
	case protocol.TypeScreenShareRequest:
		return s.dispatchScreenShare(id, envelope, sender)
	case protocol.TypeGetGameCatalog:
		if _, ok := s.state.User(id); !ok {
			return errors.New("unknown session")
		}
		message, err := protocol.NewEnvelope(protocol.TypeGameCatalog, s.state.GameCatalog())
		if err != nil {
			return err
		}
		return sender.Send(message)
	case protocol.TypeSetGameActivity:
		var request protocol.GameActivityRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		changed, err := s.state.SetGameActivity(id, request.GameID, time.Now())
		if err != nil {
			return err
		}
		if changed {
			s.broadcastState(id)
		}
	case protocol.TypeWatchRequest:
		return s.dispatchWatch(id, envelope, sender)
	case protocol.MessageType("set_channel_password"):
		var request protocol.ChannelRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionChannelEdit, administration.Resource{ChannelID: request.ChannelID}); err != nil {
			return err
		}
		if err := s.administration.SetChannelPassword(request.ChannelID, request.Password); err != nil {
			return err
		}
		s.audit("channel password changed", actorClientID, "", request.ChannelID)
		s.broadcastSnapshot()
	case protocol.TypeJoinChannel:
		var request protocol.ChannelRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if s.administration != nil {
			if err := s.administration.CheckChannelPassword(request.ChannelID, request.Password); err != nil {
				return err
			}
		}
		oldChannel := s.state.ChannelOf(id)
		if err := s.state.Join(id, request.ChannelID); err != nil {
			return err
		}
		if s.archive != nil && oldChannel != request.ChannelID {
			if oldChannel != 0 {
				s.archive.Observe(archive.Activity{Kind: archive.Left, RoomID: oldChannel, MemberID: actorClientID, At: time.Now()})
			}
			channelName := ""
			if channel, ok := s.state.Channel(request.ChannelID); ok {
				channelName = channel.Name
			}
			s.archive.Observe(archive.Activity{Kind: archive.Joined, RoomID: request.ChannelID, RoomName: channelName, MemberID: actorClientID, At: time.Now()})
		}
		s.broadcastMove(id, request.ChannelID)
		if oldChannel != request.ChannelID {
			s.broadcastRoute(oldChannel)
			s.broadcastRoomCandidates(oldChannel)
		}
		s.broadcastRoute(request.ChannelID)
		s.sendPeerCandidates(id)
		s.broadcastCandidate(id)
		s.sendCapabilities(id, sender, request.ChannelID)
		s.send(sender, protocol.TypeChatHistory, protocol.ChatHistory{ChannelID: request.ChannelID, Messages: s.state.ChatHistory(request.ChannelID)})
		if oldChannel != request.ChannelID {
			s.deleteEmptyTemporaryChannel(oldChannel)
		}
		s.logger.Info("channel joined", "session_id", id, "channel_id", request.ChannelID)
	case protocol.TypeLeaveChannel:
		oldChannel := s.state.ChannelOf(id)
		if err := s.state.Leave(id); err != nil {
			return err
		}
		if s.archive != nil {
			s.archive.Observe(archive.Activity{Kind: archive.Left, RoomID: oldChannel, MemberID: actorClientID, At: time.Now()})
		}
		s.broadcastMove(id, 0)
		s.broadcastRoute(oldChannel)
		s.broadcastRoomCandidates(oldChannel)
		s.sendCapabilities(id, sender, 0)
		s.deleteEmptyTemporaryChannel(oldChannel)
	case protocol.TypeRegisterCandidates:
		var request protocol.CandidateRegistration
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.state.SetCandidates(id, request.Candidates); err != nil {
			return err
		}
		s.broadcastCandidate(id)
		s.sendPeerCandidates(id)
	case protocol.TypeNetworkReport:
		var report protocol.NetworkReport
		if err := envelope.Decode(&report); err != nil {
			return err
		}
		if err := s.state.SetNetworkReport(id, report); err != nil {
			return err
		}
		s.broadcastRoute(s.state.ChannelOf(id))
	case protocol.TypeSetTransportMode:
		var request protocol.TransportModeRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if request.Mode != "auto" && request.Mode != "force_p2p" && request.Mode != "force_sfu" && request.Mode != "force_relay" {
			return errors.New("invalid transport mode")
		}
		if err := s.state.SetTransportMode(id, request.Mode); err != nil {
			return err
		}
		channelID := s.state.ChannelOf(id)
		s.broadcastRoute(channelID)
		s.broadcastRoomCandidates(channelID)
	case protocol.TypeSetRoomNetworkPolicy:
		return s.dispatchAdministration(id, actorClientID, envelope, sender)
	case protocol.TypeAudioQoEFeedback:
		var feedback protocol.AudioQoEFeedback
		if err := envelope.Decode(&feedback); err != nil {
			return err
		}
		targetID := feedback.PeerID
		sourceRoom := s.state.ChannelOf(id)
		if sourceRoom == 0 || s.state.ChannelOf(targetID) != sourceRoom {
			// Audio feedback is best-effort. A peer can leave between the
			// client's periodic QoE sample and this control message, which is
			// normal and must not surface as a protocol error to the remaining
			// participant.
			return nil
		}
		feedback.PeerID = id
		message, err := protocol.NewEnvelope(protocol.TypeAudioQoEFeedback, feedback)
		if err != nil {
			return err
		}
		// The target may disconnect after the room check; QoE feedback is
		// observational, so silently drop that stale sample as well.
		s.state.SendTo(targetID, message)
	case protocol.TypeSubscribeStreams:
		var request protocol.SubscriptionRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.state.SetSubscriptions(id, request.StreamIDs); err != nil {
			return err
		}
	case protocol.TypeSetMute:
		var request protocol.StateRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.state.SetMuted(id, request.Enabled); err != nil {
			return err
		}
		s.broadcastState(id)
	case protocol.TypeSetDeafen:
		var request protocol.StateRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.state.SetDeafened(id, request.Enabled); err != nil {
			return err
		}
		s.broadcastState(id)
	case protocol.TypeSetDisplayName:
		var request protocol.DisplayNameRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		request.DisplayName = strings.TrimSpace(request.DisplayName)
		if err := validateDisplayName(request.DisplayName); err != nil {
			return err
		}
		if err := s.administration.UpdateDisplayName(actorClientID, request.DisplayName, time.Now()); err != nil {
			return err
		}
		s.state.UpdateDisplayName(actorClientID, request.DisplayName)
		s.broadcastSnapshot()
		s.audit("display name changed", actorClientID, actorClientID, 0)
	case protocol.TypeSetAvatar:
		var request protocol.AvatarRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		reference, data, err := protocol.CanonicalAvatar(request.Avatar)
		if err != nil || (strings.HasPrefix(reference, protocol.CustomAvatarRefPrefix) && data == "") {
			return errors.New("invalid avatar upload")
		}
		if s.administration == nil || actorClientID == "" {
			return administration.ErrForbidden
		}
		if err := s.administration.UpdateAvatar(actorClientID, request.Avatar, time.Now()); err != nil {
			return err
		}
		user, ok := s.administration.User(actorClientID)
		if !ok {
			return errors.New("unknown client identity")
		}
		s.state.UpdateAvatar(actorClientID, user.Avatar)
		s.broadcastSnapshot()
	case protocol.TypeGetAvatar:
		var request protocol.AvatarRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if _, data, err := protocol.CanonicalAvatar(request.Avatar); err != nil || data != "" {
			return errors.New("invalid avatar reference")
		}
		data, ok := s.administration.AvatarData(request.Avatar)
		if !ok {
			return errors.New("avatar not found")
		}
		s.send(sender, protocol.TypeAvatarData, protocol.AvatarData{Avatar: request.Avatar, Data: data})
	case protocol.MessageType("query_chat"):
		return s.queryChat(id, envelope, sender)
	case protocol.TypeSendTextMessage:
		var request protocol.SendTextMessageRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		request.Text = strings.TrimSpace(request.Text)
		if request.Text == "" || len(request.Text) > protocol.MaxChatMessageBytes || !utf8.ValidString(request.Text) {
			return errors.New("message must be valid UTF-8 and 1-2048 bytes")
		}
		message, err := s.state.RecordTextMessage(id, request.Text, time.Now())
		if err != nil {
			return err
		}
		event, err := protocol.NewEnvelope(protocol.TypeTextMessage, message)
		if err != nil {
			return err
		}
		s.state.BroadcastRoom(message.ChannelID, event, 0)
		s.broadcastChatHeads(message)
		if s.archive != nil {
			s.archive.Observe(archive.Activity{Kind: archive.TextMessage, RoomID: message.ChannelID, MemberID: actorClientID, At: time.Now()})
		}
	case protocol.TypeGetRoomArchive:
		var request protocol.RoomArchiveRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if s.archive == nil {
			return errors.New("Eden Archive is unavailable")
		}
		if _, ok := s.state.Channel(request.ChannelID); !ok {
			return serverstate.ErrUnknownChannel
		}
		snapshot := s.archive.Snapshot(request.ChannelID, request.All, request.Limit)
		return s.sendBoundedArchive(sender, snapshot)
	case protocol.TypeVoiceActivity:
		if s.archive != nil && s.state.CanSpeak(id) {
			s.archive.ObserveVoice(s.state.ChannelOf(id), actorClientID, time.Now())
		}
	case protocol.TypePing:
		var ping protocol.Ping
		if err := envelope.Decode(&ping); err != nil {
			return err
		}
		if !s.state.Touch(id) {
			return serverstate.ErrUnknownSession
		}
		s.send(sender, protocol.TypePong, ping)
	case protocol.TypeRequestMediaRekey:
		epoch := s.state.MediaEpoch(id) + 1
		if epoch == 0 {
			return errors.New("media epoch exhausted; reconnect required")
		}
		salt := make([]byte, 32)
		if _, err := cryptorand.Read(salt); err != nil {
			return err
		}
		if err := s.state.PrepareMediaRekey(id, salt, epoch); err != nil {
			return err
		}
		s.send(sender, protocol.TypeMediaRekeyPrepare, protocol.MediaRekey{Epoch: epoch, Salt: base64.RawStdEncoding.EncodeToString(salt)})
	case protocol.TypeMediaRekeyAck:
		var rekey protocol.MediaRekey
		if err := envelope.Decode(&rekey); err != nil {
			return err
		}
		if err := s.state.ActivateMediaRekey(id, rekey.Epoch); err != nil {
			return err
		}
		s.send(sender, protocol.TypeMediaRekeyCommit, protocol.MediaRekey{Epoch: rekey.Epoch})
		time.AfterFunc(mediaRekeyGrace, func() {
			s.state.DropPreviousMediaEpoch(id, rekey.Epoch)
		})
		s.logger.Info("media session rekeyed", "session_id", id, "key_epoch", rekey.Epoch)
	case protocol.TypeProvisionMaster:
		if s.administration == nil {
			return errors.New("administration is unavailable")
		}
		var request protocol.ProvisionMasterRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.administration.ProvisionMaster(actorClientID, request.Token); err != nil {
			return err
		}
		s.state.UpdateServerRole(actorClientID, string(administration.RoleMaster))
		s.sendCapabilities(id, sender, s.state.ChannelOf(id))
		s.audit("master provisioned", actorClientID, actorClientID, 0)
		s.broadcastSnapshot()
	case protocol.TypeCreateChannel, protocol.TypeEditChannel, protocol.TypeDeleteChannel, protocol.TypeSetChannelAdmin, protocol.TypeSetServerRole, protocol.TypeAdminMute, protocol.TypeMoveUser, protocol.TypeKickChannel, protocol.TypeKickServer, protocol.TypeBan, protocol.TypeUnban, protocol.TypeListBans:
		return s.dispatchAdministration(id, actorClientID, envelope, sender)
	default:
		return errors.New("unsupported message type")
	}
	return nil
}

func (s *Server) sendPeerCandidates(id uint64) {
	if !s.state.PeerCandidateFanoutAllowed(s.state.ChannelOf(id)) {
		return
	}
	for _, peer := range s.state.PeerCandidates(id) {
		if msg, err := protocol.NewEnvelope(protocol.TypePeerCandidates, peer); err == nil {
			s.state.SendTo(id, msg)
		}
	}
}

func (s *Server) resolveTarget(request protocol.TargetRequest) (*serverstate.Session, error) {
	if request.SessionID != 0 {
		target, ok := s.state.SessionByID(request.SessionID)
		if !ok {
			return nil, errors.New("target is offline")
		}
		if target.ClientID != request.ClientID {
			return nil, errors.New("target session does not match client identity")
		}
		return target, nil
	}
	targets := s.state.SessionsByClientID(request.ClientID)
	if len(targets) == 0 {
		return nil, errors.New("target is offline")
	}
	if len(targets) != 1 {
		return nil, errors.New("target identity has multiple sessions; session_id is required")
	}
	return targets[0], nil
}
func (s *Server) broadcastCandidate(id uint64) {
	channelID := s.state.ChannelOf(id)
	if !s.state.PeerCandidateFanoutAllowed(channelID) {
		return
	}
	self, ok := s.state.CandidateSnapshot(id)
	if !ok {
		return
	}
	msg, _ := protocol.NewEnvelope(protocol.TypePeerCandidates, self)
	s.state.BroadcastRoom(channelID, msg, id)
}
func (s *Server) broadcastRoomCandidates(channelID uint64) {
	if !s.state.PeerCandidateFanoutAllowed(channelID) {
		return
	}
	for _, id := range s.state.RoomSessionIDs(channelID) {
		s.sendPeerCandidates(id)
	}
}
func (s *Server) broadcastRoute(channelID uint64) {
	if channelID == 0 {
		return
	}
	s.routeMu.Lock()
	defer s.routeMu.Unlock()
	update := s.state.EvaluateRoom(channelID)
	if previous, ok := s.lastRoute[channelID]; ok && previous == update {
		return
	}
	s.lastRoute[channelID] = update
	msg, _ := protocol.NewEnvelope(protocol.TypeRouteUpdate, update)
	s.state.BroadcastRoom(channelID, msg, 0)
	s.logger.Info("room route evaluated", "room_id", channelID, "topology", update.Topology, "transport", update.Transport, "reason", update.Reason)
}

func (s *Server) broadcastMove(id, channelID uint64) {
	s.cancelRoomSocialOnMove(id)
	s.leaveScreenShare(id)
	s.leaveWatchOnMove(id, channelID)
	event, _ := protocol.NewEnvelope(protocol.TypeUserMoved, protocol.UserMoved{SessionID: id, ChannelID: channelID})
	s.state.Broadcast(event, 0)
}
func (s *Server) broadcastState(id uint64) {
	user, ok := s.state.User(id)
	if !ok {
		return
	}
	event, _ := protocol.NewEnvelope(protocol.TypeUserStateChanged, protocol.UserStateChanged{GameActivity: user.GameActivity, SessionID: id, Muted: user.Muted, Deafened: user.Deafened, AdminMuted: user.AdminMuted, ServerMuted: user.ServerMuted, ChannelMuted: user.ChannelMuted})
	s.state.Broadcast(event, 0)
}
func (s *Server) disconnect(id uint64, closeConnection bool) {
	s.chatQueryMu.Lock()
	delete(s.chatLastQuery, id)
	s.chatQueryMu.Unlock()
	s.leaveScreenShare(id)
	session, ok := s.state.RemoveSession(id)
	s.removeRoomSocial(id)
	if !ok {
		return
	}
	s.leaveWatchOnMove(id, 0)
	if s.archive != nil && session.ChannelID != 0 {
		s.archive.Observe(archive.Activity{Kind: archive.Left, RoomID: session.ChannelID, MemberID: session.ClientID, At: time.Now()})
	}
	event, _ := protocol.NewEnvelope(protocol.TypeUserLeft, protocol.UserLeft{SessionID: id})
	s.state.Broadcast(event, 0)
	s.broadcastRoute(session.ChannelID)
	s.broadcastRoomCandidates(session.ChannelID)
	s.deleteEmptyTemporaryChannel(session.ChannelID)
	if closeConnection && session.Sender != nil {
		if closer, ok := session.Sender.(interface{ Close() error }); ok {
			_ = closer.Close()
		}
	}
	s.logger.Info("session disconnected", "session_id", id)
}

func (s *Server) deleteEmptyTemporaryChannel(channelID uint64) {
	if channelID == 0 {
		return
	}
	channel, removed := s.state.DeleteEmptyTemporaryChannel(channelID)
	if !removed {
		return
	}
	if s.administration != nil {
		if err := s.administration.DeleteChannel(channelID); err != nil {
			s.logger.Error("temporary channel persistence cleanup failed", "channel_id", channelID, "error", err)
		}
	}
	s.logger.Info("empty temporary channel deleted", "channel_id", channelID, "channel_name", channel.Name)
	s.broadcastSnapshot()
	s.broadcastCapabilities()
}

func (s *Server) disconnectWithError(sessionID uint64, sender serverstate.Sender, code, message string) {
	s.send(sender, protocol.TypeError, protocol.Error{Code: code, Message: message})
	s.disconnect(sessionID, true)
}

func (s *Server) observeArchiveMove(clientID string, oldChannel, newChannel uint64) {
	if s.archive == nil || clientID == "" || oldChannel == newChannel {
		return
	}
	now := time.Now()
	if oldChannel != 0 {
		s.archive.Observe(archive.Activity{Kind: archive.Left, RoomID: oldChannel, MemberID: clientID, At: now})
	}
	if newChannel != 0 {
		name := ""
		if channel, ok := s.state.Channel(newChannel); ok {
			name = channel.Name
		}
		s.archive.Observe(archive.Activity{Kind: archive.Joined, RoomID: newChannel, RoomName: name, MemberID: clientID, At: now})
	}
}

func (s *Server) send(sender serverstate.Sender, kind protocol.MessageType, value any) {
	if sender == nil {
		return
	}
	msg, err := protocol.NewEnvelope(kind, value)
	if err == nil {
		_ = sender.Send(msg)
	}
}

func (s *Server) sendBoundedArchive(sender serverstate.Sender, snapshot archive.RoomSnapshot) error {
	// Private aggregation maps are not a wire API. Sending all historical pairs
	// or translations could exceed the control frame cap and disconnect voice.
	snapshot.ActiveDayCount = len(snapshot.Stats.ActiveDays)
	snapshot.HistoricalMemberCount = len(snapshot.Stats.Members)
	// Rust's default collection deserializers accept missing fields or empty
	// collections, not JSON null. Keep private maps empty on the wire.
	snapshot.Stats.ActiveDays = map[string]bool{}
	snapshot.Stats.ActiveHours = map[int]uint64{}
	snapshot.Stats.Members = map[string]archive.MemberStats{}
	snapshot.Stats.Pairs = map[string]archive.PairStats{}
	snapshot.Events = append([]archive.Event{}, snapshot.Events...)
	for i := range snapshot.Events {
		snapshot.Events[i].Metadata = nil
	}
	for {
		snapshot.ParticipantProfiles = s.archiveParticipantProfiles(snapshot.Events)
		message, err := protocol.NewEnvelope(protocol.TypeRoomArchive, snapshot)
		if err != nil {
			return err
		}
		encoded, err := json.Marshal(message)
		if err != nil {
			return err
		}
		if len(encoded)+1 <= protocol.MaxControlMessageBytes {
			if sender != nil {
				return sender.Send(message)
			}
			return nil
		}
		if len(snapshot.Events) <= 1 {
			return errors.New("archive entry exceeds control frame limit")
		}
		snapshot.Events = snapshot.Events[:len(snapshot.Events)-1]
		snapshot.HasMore = true
	}
}

func (s *Server) archiveParticipantProfiles(events []archive.Event) map[string]archive.ParticipantProfile {
	needed := make(map[string]bool)
	for _, event := range events {
		for _, clientID := range event.TriggeredBy {
			needed[clientID] = clientID != ""
		}
		for _, clientID := range event.Participants {
			needed[clientID] = clientID != ""
		}
	}
	live := make(map[string]protocol.User)
	for _, user := range s.state.Snapshot().Users {
		if user.ClientID != "" {
			live[user.ClientID] = user
		}
	}
	profiles := make(map[string]archive.ParticipantProfile, len(needed))
	for clientID, include := range needed {
		if !include {
			continue
		}
		profile := archive.ParticipantProfile{ClientID: clientID, DisplayName: "Unknown member"}
		if user, ok := live[clientID]; ok {
			profile.DisplayName = user.Nickname
			profile.Avatar = user.Avatar
		}
		if s.administration != nil {
			if user, ok := s.administration.User(clientID); ok {
				profile.DisplayName = user.DisplayName
				profile.Avatar = user.Avatar
			}
		}
		profiles[clientID] = profile
	}
	return profiles
}
func (s *Server) sendError(sender *connSender, code, message string) {
	s.send(sender, protocol.TypeError, protocol.Error{Code: code, Message: message})
}

func (s *Server) sendBanList(sender serverstate.Sender, offset, limit int) {
	bans := s.administration.Bans(time.Now())
	total := len(bans)
	if total == 0 {
		offset = 0
	} else if offset >= total {
		offset = (total - 1) / limit * limit
	}
	end := min(offset+limit, total)
	records := make([]protocol.BanRecord, 0, end-offset)
	for _, ban := range bans[offset:end] {
		displayName := ""
		if user, ok := s.administration.User(ban.ClientID); ok {
			displayName = user.DisplayName
		}
		records = append(records, protocol.BanRecord{
			ClientID: ban.ClientID, DisplayName: displayName, Reason: ban.Reason,
			CreatedAt: ban.CreatedAt, ExpiresAt: ban.ExpiresAt, OperatorClientID: ban.OperatorClientID,
		})
	}
	s.send(sender, protocol.TypeBanList, protocol.BanList{Bans: records, Offset: offset, Total: total})
}

func (s *Server) authorize(subject string, action administration.Action, resource administration.Resource) error {
	if s.administration == nil {
		return administration.ErrForbidden
	}
	return s.administration.Authorize(subject, action, resource)
}

func (s *Server) broadcastSnapshot() {
	message, err := protocol.NewEnvelope(protocol.TypeServerState, s.snapshot())
	if err == nil {
		s.state.Broadcast(message, 0)
	}
}

func (s *Server) snapshot() protocol.ServerState {
	snapshot := s.state.Snapshot()
	snapshot.ChatHeads = s.state.ChatHeads()
	if s.announcements != nil {
		announcement := s.announcements.Snapshot()
		snapshot.Announcement = announcement.Published
		snapshot.AnnouncementRevision = announcement.Revision
	}
	if s.updates != nil {
		snapshot.ClientUpdates = s.updates.Published()
	}
	if s.administration != nil {
		snapshot.PasswordRoomsSupported = true
		snapshot.LockedChannelIDs = s.administration.LockedChannelIDs()
		for index := range snapshot.Users {
			snapshot.Users[index].ChannelRole = string(s.administration.ChannelRole(snapshot.Users[index].ChannelID, snapshot.Users[index].ClientID))
		}
	}
	return snapshot
}

func (s *Server) fallbackChannel(excluding uint64) uint64 {
	for _, channel := range s.state.Snapshot().Channels {
		if channel.ID != excluding {
			return channel.ID
		}
	}
	return 0
}

func validateChannel(channel protocol.Channel) error {
	channel.Name = strings.TrimSpace(channel.Name)
	if channel.ID == 0 || channel.Name == "" || len(channel.Name) > protocol.MaxNicknameBytes || !utf8.ValidString(channel.Name) {
		return errors.New("channel name must be valid UTF-8 and 1-64 bytes")
	}
	if channel.UserLimit > protocol.MaxChannelUserLimit {
		return errors.New("channel user limit must be between 0 and 10000")
	}
	switch channel.AudioQualityPolicy {
	case "", "auto",
		"quality_1", "quality_2", "quality_3", "quality_4", "quality_5",
		"quality_6", "quality_7", "quality_8", "quality_9", "quality_10",
		"fixed_32k", "fixed_48k", "fixed_64k", "fixed_96k":
	default:
		return errors.New("channel audio quality must be auto or quality level 1-10")
	}
	return nil
}

func validateDisplayName(displayName string) error {
	if displayName == "" || len(displayName) > protocol.MaxNicknameBytes || !utf8.ValidString(displayName) {
		return errors.New("display name must be valid UTF-8 and 1-64 bytes")
	}
	return nil
}

func validateNewChannel(channel protocol.Channel) error {
	channel.ID = 1
	return validateChannel(channel)
}

func (s *Server) audit(event, operator, target string, channelID uint64, extra ...any) {
	attributes := []any{"event", event, "operator_client_id", operator, "target_client_id", target, "channel_id", channelID}
	attributes = append(attributes, extra...)
	s.logger.Info("administration audit", attributes...)
	s.auditMu.Lock()
	defer s.auditMu.Unlock()
	s.auditEvents = append(s.auditEvents, AdminAudit{At: time.Now(), Event: event, Operator: operator, Target: target, ChannelID: channelID})
	if len(s.auditEvents) > 200 {
		s.auditEvents = append([]AdminAudit(nil), s.auditEvents[len(s.auditEvents)-200:]...)
	}
}

func (s *Server) capabilities(clientID string, channelID uint64) []string {
	if s.administration == nil || clientID == "" {
		return nil
	}
	actions := []administration.Action{administration.ActionChannelCreate, administration.ActionChannelEdit, administration.ActionChannelDelete, administration.ActionChannelRoleSet, administration.ActionNetworkPolicySet, administration.ActionMemberMute, administration.ActionMemberMove, administration.ActionMemberKickChannel, administration.ActionMemberKickServer, administration.ActionMemberBan, administration.ActionMemberUnban, administration.ActionMemberBanList, administration.ActionServerRoleSet, administration.ActionWatchManage}
	result := make([]string, 0, len(actions))
	for _, action := range actions {
		if s.administration.Authorize(clientID, action, administration.Resource{ChannelID: channelID}) == nil {
			result = append(result, string(action))
		}
	}
	return result
}

func (s *Server) networkPolicyChannelIDs(clientID string) []uint64 {
	if s.administration == nil || clientID == "" {
		return nil
	}
	channels := s.administration.Channels()
	result := make([]uint64, 0, len(channels))
	for _, channel := range channels {
		if s.administration.Authorize(clientID, administration.ActionNetworkPolicySet, administration.Resource{ChannelID: channel.ID}) == nil {
			result = append(result, channel.ID)
		}
	}
	return result
}

func (s *Server) sendCapabilities(sessionID uint64, sender serverstate.Sender, channelID uint64) {
	if sender == nil {
		return
	}
	clientID := s.state.ClientID(sessionID)
	role := ""
	if s.administration != nil {
		if user, ok := s.administration.User(clientID); ok {
			role = string(user.ServerRole)
		}
	}
	message, err := protocol.NewEnvelope(protocol.TypeCapabilities, protocol.Capabilities{
		ServerRole:              role,
		Actions:                 s.capabilities(clientID, channelID),
		NetworkPolicyChannelIDs: s.networkPolicyChannelIDs(clientID),
	})
	if err == nil {
		_ = sender.Send(message)
	}
}

func (s *Server) broadcastCapabilities() {
	for _, user := range s.state.Snapshot().Users {
		if session, ok := s.state.SessionByID(user.SessionID); ok {
			s.sendCapabilities(session.ID, session.Sender, session.ChannelID)
		}
	}
}
