package serverstate

import (
	cryptorand "crypto/rand"
	"encoding/binary"
	"errors"
	"math"
	"net"
	"net/netip"
	"sort"
	"sync"
	"time"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
)

var (
	ErrUnknownSession  = errors.New("unknown session")
	ErrP2PNotSupported = errors.New("p2p not supported for current room")
	ErrUnknownChannel  = errors.New("unknown channel")
	ErrChannelFull     = errors.New("channel user limit reached")
	ErrChatUnavailable = errors.New("chat persistence unavailable; message was not sent")
)

const maxHostCandidates = 32

type Sender interface{ Send(protocol.Envelope) error }

type ChatStore interface {
	Append(protocol.TextMessage) error
	Recent(uint64, int) ([]protocol.TextMessage, error)
	LastID() uint64
}

type Session struct {
	ChatQuerySupported bool
	RoomSocialSupported bool
	RoomGeneration      uint64
	Away                bool
	LastReactionAt      time.Time
	ClientVersion       string
	ClientPlatform      string
	GameActivity        *protocol.GameActivity
	GameActivityAt      time.Time
	ID                  uint64
	Nickname            string
	ClientID            string
	ServerRole          string
	Avatar              string
	ClientEphemeralKey  string
	ChannelID           uint64
	UDPAddr             *net.UDPAddr
	Muted               bool
	AdminMuted          bool // server-wide administrative mute
	ChannelMutedIn      uint64
	Deafened            bool
	ConnectedAt         time.Time
	LastHeartbeat       time.Time
	Sender              Sender
	Candidates          []protocol.Candidate
	CandidateGeneration uint64
	LastCandidateNotice time.Time
	TransportMode       string
	Reports             map[uint64]protocol.NetworkReport
	ReportTimes         map[uint64]time.Time
	Subscriptions       map[uint64]bool
	MediaInbound        *security.MediaCipher
	MediaOutbound       *security.MediaCipher
}

type roomRoutingState struct {
	Topology      string
	LastChanged   time.Time
	CooldownUntil time.Time
}
type RuntimeStats struct {
	OnlineUsers   int
	Rooms         int
	P2PSessions   int
	SFUSessions   int
	RelaySessions int
}

type sceneDecision struct {
	current   protocol.RoomScene
	candidate string
	since     time.Time
}
type State struct {
	gameCatalog *protocol.GameCatalog
	scenes      map[uint64]sceneDecision
	// Chat ordering and disk I/O must never hold the voice path's mu.
	chatMu        sync.Mutex
	chatStore     ChatStore
	chatError     error
	mu            sync.RWMutex
	serverName    string
	channels      map[uint64]protocol.Channel
	sessions      map[uint64]*Session
	roomMembers   map[uint64]map[uint64]*Session
	nextSessionID uint64
	network       config.Network
	roomTopology  map[uint64]roomRoutingState
	roomPolicy    map[uint64]string
	chatHistory   map[uint64][]protocol.TextMessage
	nextMessageID uint64
	lastChatAt    map[uint64]time.Time
}

func New(cfg config.Config) *State {
	channels := make(map[uint64]protocol.Channel, len(cfg.Channels))
	roomPolicy := make(map[uint64]string, len(cfg.Channels))
	for _, ch := range cfg.Channels {
		policy := ch.NetworkPolicy
		if policy == "" {
			policy = "auto"
		}
		userLimit := uint32(0)
		if ch.UserLimit > 0 {
			userLimit = uint32(ch.UserLimit)
		}
		audioQualityPolicy := ch.AudioQualityPolicy
		if audioQualityPolicy == "" {
			audioQualityPolicy = "auto"
		}
		channels[ch.ID] = protocol.Channel{ID: ch.ID, ParentID: ch.ParentID, Name: ch.Name, SortOrder: ch.SortOrder, UserLimit: userLimit, NetworkPolicy: policy, AudioQualityPolicy: audioQualityPolicy}
		roomPolicy[ch.ID] = policy
	}
	return &State{gameCatalog: &protocol.GameCatalog{Version: 1, Games: protocol.DefaultGames()}, scenes: map[uint64]sceneDecision{}, serverName: cfg.ServerName, channels: channels, sessions: make(map[uint64]*Session), nextSessionID: 10000, network: cfg.Network, roomTopology: make(map[uint64]roomRoutingState), roomPolicy: roomPolicy, chatHistory: make(map[uint64][]protocol.TextMessage), lastChatAt: make(map[uint64]time.Time)}
}

func (s *State) AddSession(nickname string, sender Sender) *Session {
	return s.AddAuthenticatedSession(nickname, "", "", "", sender, nil, nil)
}

func (s *State) AddAuthenticatedSession(nickname, clientID, serverRole, clientEphemeralKey string, sender Sender, inbound, outbound *security.MediaCipher) *Session {
	s.mu.Lock()
	defer s.mu.Unlock()
	id := s.nextIDLocked()
	now := time.Now()
	session := &Session{ID: id, Nickname: nickname, ClientID: clientID, ServerRole: serverRole, ClientEphemeralKey: clientEphemeralKey, ConnectedAt: now, LastHeartbeat: now, Sender: sender, TransportMode: "auto", Reports: make(map[uint64]protocol.NetworkReport), ReportTimes: make(map[uint64]time.Time), Subscriptions: make(map[uint64]bool), MediaInbound: inbound, MediaOutbound: outbound}
	s.sessions[session.ID] = session
	return cloneSession(session)
}

func (s *State) MediaSecurity(id uint64) (inbound, outbound *security.MediaCipher, ok bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	if session == nil || session.MediaInbound == nil || session.MediaOutbound == nil {
		return nil, nil, false
	}
	return session.MediaInbound, session.MediaOutbound, true
}

func (s *State) PrepareMediaRekey(id uint64, salt []byte, epoch uint16) error {
	inbound, outbound, ok := s.MediaSecurity(id)
	if !ok {
		return ErrUnknownSession
	}
	if err := inbound.PrepareRekey(salt, epoch); err != nil {
		return err
	}
	return outbound.PrepareRekey(salt, epoch)
}
func (s *State) ActivateMediaRekey(id uint64, epoch uint16) error {
	inbound, outbound, ok := s.MediaSecurity(id)
	if !ok {
		return ErrUnknownSession
	}
	if err := inbound.ActivatePrepared(epoch); err != nil {
		return err
	}
	return outbound.ActivatePrepared(epoch)
}
func (s *State) DropPreviousMediaEpoch(id uint64, currentEpoch uint16) {
	inbound, outbound, ok := s.MediaSecurity(id)
	if !ok {
		return
	}
	inbound.DropPreviousIfCurrent(currentEpoch)
	outbound.DropPreviousIfCurrent(currentEpoch)
}
func (s *State) MediaEpoch(id uint64) uint16 {
	_, outbound, ok := s.MediaSecurity(id)
	if !ok {
		return 0
	}
	return outbound.Epoch()
}

func (s *State) nextIDLocked() uint64 {
	var bytes [8]byte
	for range 8 {
		if _, err := cryptorand.Read(bytes[:]); err != nil {
			break
		}
		id := binary.BigEndian.Uint64(bytes[:])
		if id != 0 {
			if _, exists := s.sessions[id]; !exists {
				return id
			}
		}
	}
	for {
		s.nextSessionID++
		if s.nextSessionID != 0 {
			if _, exists := s.sessions[s.nextSessionID]; !exists {
				return s.nextSessionID
			}
		}
	}
}

func (s *State) RemoveSession(id uint64) (*Session, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, exists := s.sessions[id]
	removed := cloneSession(session)
	if exists {
		s.clearAwayLocked(session)
		s.removeRoomMemberLocked(session)
		delete(s.sessions, id)
		delete(s.lastChatAt, id)
		s.clearPeerStateLocked(id)
		if session.ChannelID != 0 && s.roomSizeLocked(session.ChannelID) == 0 {
			delete(s.roomTopology, session.ChannelID)
		}
		if session.MediaInbound != nil {
			session.MediaInbound.Destroy()
			session.MediaInbound = nil
		}
		if session.MediaOutbound != nil {
			session.MediaOutbound.Destroy()
			session.MediaOutbound = nil
		}
		removed.MediaInbound = nil
		removed.MediaOutbound = nil
	}
	return removed, exists
}

// Room membership changes under the same lock as session state. Eligibility
// (deafened, subscriptions, UDP address and keys) is always read live.
func (s *State) removeRoomMemberLocked(session *Session) {
	members := s.roomMembers[session.ChannelID]
	delete(members, session.ID)
	if len(members) == 0 {
		delete(s.roomMembers, session.ChannelID)
	}
}
func (s *State) setSessionChannelLocked(session *Session, channel uint64) {
	if session.ChannelID != channel {
		session.RoomGeneration++
		s.clearAwayLocked(session)
	}
	s.removeRoomMemberLocked(session)
	session.ChannelID = channel
	if channel == 0 {
		return
	}
	if s.roomMembers == nil {
		s.roomMembers = make(map[uint64]map[uint64]*Session)
	}
	if s.roomMembers[channel] == nil {
		s.roomMembers[channel] = make(map[uint64]*Session)
	}
	s.roomMembers[channel][session.ID] = session
}

func (s *State) Join(id, channelID uint64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	channel, exists := s.channels[channelID]
	if !exists {
		return ErrUnknownChannel
	}
	session, exists := s.sessions[id]
	if !exists {
		return ErrUnknownSession
	}
	if session.ChannelID != channelID {
		if channel.UserLimit > 0 && s.roomSizeLocked(channelID) >= int(channel.UserLimit) {
			return ErrChannelFull
		}
		oldChannelID := session.ChannelID
		s.clearPeerStateLocked(id)
		if oldChannelID != 0 && s.roomSizeLocked(oldChannelID) == 1 {
			delete(s.roomTopology, oldChannelID)
		}
	}
	s.setSessionChannelLocked(session, channelID)
	return nil
}

func (s *State) Leave(id uint64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, exists := s.sessions[id]
	if !exists {
		return ErrUnknownSession
	}
	oldChannelID := session.ChannelID
	s.clearPeerStateLocked(id)
	s.setSessionChannelLocked(session, 0)
	if oldChannelID != 0 && s.roomSizeLocked(oldChannelID) == 0 {
		delete(s.roomTopology, oldChannelID)
	}
	return nil
}

func (s *State) clearPeerStateLocked(id uint64) {
	if session := s.sessions[id]; session != nil {
		session.Reports = make(map[uint64]protocol.NetworkReport)
		session.ReportTimes = make(map[uint64]time.Time)
		session.Subscriptions = make(map[uint64]bool)
	}
	for peerID, peer := range s.sessions {
		if peerID == id {
			continue
		}
		delete(peer.Reports, id)
		delete(peer.ReportTimes, id)
		delete(peer.Subscriptions, id)
	}
}

func (s *State) SetMuted(id uint64, muted bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, ok := s.sessions[id]
	if !ok {
		return ErrUnknownSession
	}
	session.Muted = muted
	if !muted {
		session.Deafened = false
		s.clearAwayLocked(session)
	}
	return nil
}

func (s *State) SetAdminMuted(id uint64, muted bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return ErrUnknownSession
	}
	session.AdminMuted = muted
	return nil
}

func (s *State) SetChannelMuted(id, channelID uint64, muted bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return ErrUnknownSession
	}
	if session.ChannelID != channelID || channelID == 0 {
		return errors.New("target is not in the channel mute scope")
	}
	if muted {
		session.ChannelMutedIn = channelID
	} else if session.ChannelMutedIn == channelID {
		session.ChannelMutedIn = 0
	}
	return nil
}

func (s *State) UpdateDisplayName(clientID, displayName string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, session := range s.sessions {
		if session.ClientID == clientID {
			session.Nickname = displayName
		}
	}
}

func (s *State) UpdateAvatar(clientID, avatar string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, session := range s.sessions {
		if session.ClientID == clientID {
			session.Avatar = avatar
		}
	}
}

func (s *State) SetAvatar(id uint64, avatar string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return ErrUnknownSession
	}
	session.Avatar = avatar
	return nil
}

func (s *State) RecordTextMessage(id uint64, text string, now time.Time) (protocol.TextMessage, error) {
	return s.recordMessage(id, text, nil, 0, 0, now)
}

func (s *State) RecordImageMessage(id, channel, generation uint64, image protocol.ChatImage, now time.Time) (protocol.TextMessage, error) {
	return s.recordMessage(id, "[图片]", &image, channel, generation, now)
}

func (s *State) recordMessage(id uint64, text string, image *protocol.ChatImage, channel, generation uint64, now time.Time) (protocol.TextMessage, error) {
	s.chatMu.Lock()
	defer s.chatMu.Unlock()
	if s.chatError != nil {
		return protocol.TextMessage{}, ErrChatUnavailable
	}
	s.mu.Lock()
	session := s.sessions[id]
	if session == nil {
		s.mu.Unlock()
		return protocol.TextMessage{}, ErrUnknownSession
	}
	if image != nil && (session.ChannelID != channel || session.RoomGeneration != generation || session.ClientID == "") {
		s.mu.Unlock()
		return protocol.TextMessage{}, errors.New("room changed; image cancelled")
	}
	if session.ChannelID == 0 {
		s.mu.Unlock()
		return protocol.TextMessage{}, errors.New("join a channel before sending messages")
	}
	if last := s.lastChatAt[id]; !last.IsZero() && now.Sub(last) < 250*time.Millisecond {
		s.mu.Unlock()
		return protocol.TextMessage{}, errors.New("messages are being sent too quickly")
	}
	if s.nextMessageID == math.MaxUint64 {
		s.mu.Unlock()
		return protocol.TextMessage{}, ErrChatUnavailable
	}
	s.lastChatAt[id] = now
	s.nextMessageID++
	message := protocol.TextMessage{Image: image, ID: s.nextMessageID, ChannelID: session.ChannelID, SenderSessionID: session.ID, SenderClientID: session.ClientID, SenderName: session.Nickname, SenderAvatar: session.Avatar, Text: text, SentAtUnixMS: now.UnixMilli()}
	s.mu.Unlock()
	if s.chatStore != nil {
		if err := s.chatStore.Append(message); err != nil {
			return protocol.TextMessage{}, ErrChatUnavailable
		}
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.channels[message.ChannelID]; !exists {
		return message, nil
	}
	history := append(s.chatHistory[message.ChannelID], message)
	if len(history) > 100 {
		history = append([]protocol.TextMessage(nil), history[len(history)-100:]...)
	}
	s.chatHistory[message.ChannelID] = history
	return message, nil
}

// RestoreChat is called before accepting connections. Failure disables chat
// writes rather than silently falling back to volatile history; voice can run.
func (s *State) RestoreChat(store ChatStore) error {
	s.chatMu.Lock()
	defer s.chatMu.Unlock()
	s.chatError = ErrChatUnavailable
	if store == nil {
		return ErrChatUnavailable
	}
	s.mu.RLock()
	channels := make([]uint64, 0, len(s.channels))
	for id := range s.channels {
		channels = append(channels, id)
	}
	s.mu.RUnlock()
	histories := make(map[uint64][]protocol.TextMessage, len(channels))
	for _, id := range channels {
		messages, err := store.Recent(id, 100)
		if err != nil {
			return err
		}
		histories[id] = messages
	}
	lastID := store.LastID()
	s.mu.Lock()
	s.chatHistory = histories
	s.nextMessageID = lastID
	s.mu.Unlock()
	s.chatStore = store
	s.chatError = nil
	return nil
}

func (s *State) ChatHistory(channelID uint64) []protocol.TextMessage {
	s.mu.RLock()
	defer s.mu.RUnlock()
	// A non-nil empty slice is intentional: JSON must contain `messages: []`.
	// Encoding a nil slice as `null` is incompatible with clients expecting a list.
	return append([]protocol.TextMessage{}, s.chatHistory[channelID]...)
}

func (s *State) Move(id, channelID uint64) error { return s.Join(id, channelID) }

func (s *State) SessionByClientID(clientID string) (*Session, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, session := range s.sessions {
		if session.ClientID == clientID {
			return cloneSession(session), true
		}
	}
	return nil, false
}

func (s *State) SessionByID(id uint64) (*Session, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session, ok := s.sessions[id]
	return cloneSession(session), ok
}

func (s *State) SessionsByClientID(clientID string) []*Session {
	s.mu.RLock()
	defer s.mu.RUnlock()
	result := make([]*Session, 0)
	for _, session := range s.sessions {
		if session.ClientID == clientID {
			result = append(result, cloneSession(session))
		}
	}
	sort.Slice(result, func(i, j int) bool { return result[i].ID < result[j].ID })
	return result
}

func (s *State) ClientID(id uint64) string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if session := s.sessions[id]; session != nil {
		return session.ClientID
	}
	return ""
}

func (s *State) UpdateServerRole(clientID, role string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, session := range s.sessions {
		if session.ClientID == clientID {
			session.ServerRole = role
		}
	}
}

func (s *State) CreateChannel(channel protocol.Channel) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.channels[channel.ID]; exists {
		return errors.New("channel already exists")
	}
	if channel.NetworkPolicy == "" {
		channel.NetworkPolicy = "auto"
	}
	if channel.AudioQualityPolicy == "" {
		channel.AudioQualityPolicy = "auto"
	}
	s.channels[channel.ID] = channel
	s.roomPolicy[channel.ID] = channel.NetworkPolicy
	return nil
}
func (s *State) EditChannel(channel protocol.Channel) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	existing, exists := s.channels[channel.ID]
	if !exists {
		return ErrUnknownChannel
	}
	channel.NetworkPolicy = existing.NetworkPolicy
	if channel.AudioQualityPolicy == "" {
		channel.AudioQualityPolicy = existing.AudioQualityPolicy
	}
	if !channel.TemporarySpecified {
		channel.Temporary = existing.Temporary
	}
	s.channels[channel.ID] = channel
	return nil
}

func (s *State) DeleteEmptyTemporaryChannel(channelID uint64) (protocol.Channel, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	channel, exists := s.channels[channelID]
	if !exists || !channel.Temporary || s.roomSizeLocked(channelID) != 0 {
		return protocol.Channel{}, false
	}
	for _, candidate := range s.channels {
		if candidate.ParentID == channelID {
			return protocol.Channel{}, false
		}
	}
	delete(s.channels, channelID)
	delete(s.roomTopology, channelID)
	delete(s.roomPolicy, channelID)
	delete(s.chatHistory, channelID)
	return channel, true
}
func (s *State) DeleteChannel(channelID, fallbackID uint64) ([]uint64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.channels[channelID]; !exists {
		return nil, ErrUnknownChannel
	}
	if channelID == fallbackID {
		return nil, errors.New("cannot delete fallback channel")
	}
	if _, exists := s.channels[fallbackID]; !exists {
		return nil, errors.New("fallback channel does not exist")
	}
	var moved []uint64
	for id, session := range s.sessions {
		if session.ChannelID == channelID {
			s.clearPeerStateLocked(id)
			s.setSessionChannelLocked(session, fallbackID)
			moved = append(moved, id)
		}
	}
	delete(s.channels, channelID)
	delete(s.roomTopology, channelID)
	delete(s.roomPolicy, channelID)
	delete(s.chatHistory, channelID)
	return moved, nil
}

func (s *State) SetDeafened(id uint64, deafened bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, ok := s.sessions[id]
	if !ok {
		return ErrUnknownSession
	}
	session.Deafened = deafened
	if deafened {
		session.Muted = true
	}
	return nil
}

func (s *State) Touch(id uint64) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if session := s.sessions[id]; session != nil {
		session.LastHeartbeat = time.Now()
		return true
	}
	return false
}

func (s *State) RegisterUDP(id uint64, addr *net.UDPAddr) error {
	_, err := s.RefreshUDP(id, addr)
	return err
}

// RefreshUDP updates the server-reflexive endpoint after NAT rebinding. The boolean
// is true only when the endpoint changed, so keepalive registrations stay cheap.
func (s *State) RefreshUDP(id uint64, addr *net.UDPAddr) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, ok := s.sessions[id]
	if !ok {
		return false, ErrUnknownSession
	}
	if session.UDPAddr != nil && session.UDPAddr.IP.Equal(addr.IP) && session.UDPAddr.Port == addr.Port {
		return false, nil
	}
	session.UDPAddr = cloneUDPAddr(addr)
	session.CandidateGeneration++
	// A server-reflexive endpoint change invalidates both directions of the
	// previously-qualified direct path. Force room evaluation back through the
	// warm SFU path until clients qualify the new candidate generation.
	session.Reports = make(map[uint64]protocol.NetworkReport)
	session.ReportTimes = make(map[uint64]time.Time)
	for peerID, peer := range s.sessions {
		if peerID != id {
			delete(peer.Reports, id)
			delete(peer.ReportTimes, id)
		}
	}
	kept := session.Candidates[:0]
	for _, candidate := range session.Candidates {
		if candidate.Kind != "server_reflexive" {
			kept = append(kept, candidate)
		}
	}
	session.Candidates = kept
	session.Candidates = upsertCandidate(session.Candidates, protocol.Candidate{Kind: "server_reflexive", Address: addr.String(), Priority: 100})
	return true, nil
}

// ClaimCandidateNotice bounds control-plane fanout caused by a client or NAT
// that repeatedly changes its UDP source port. The media binding still moves
// immediately; only redundant peer-candidate announcements are coalesced.
func (s *State) ClaimCandidateNotice(id uint64, now time.Time, interval time.Duration) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return false
	}
	if !session.LastCandidateNotice.IsZero() && now.Sub(session.LastCandidateNotice) < interval {
		return false
	}
	session.LastCandidateNotice = now
	return true
}

func (s *State) SetCandidates(id uint64, candidates []protocol.Candidate) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return ErrUnknownSession
	}
	validated := make([]protocol.Candidate, 0, maxHostCandidates+1)
	for _, candidate := range candidates {
		if len(validated) >= maxHostCandidates {
			break
		}
		if candidate.Kind != "host" && candidate.Kind != "ipv6" {
			continue
		}
		addr, err := netip.ParseAddrPort(candidate.Address)
		if err != nil || addr.Port() == 0 || addr.Addr().IsUnspecified() || addr.Addr().IsMulticast() {
			continue
		}
		if (candidate.Kind == "host" && !addr.Addr().Is4()) || (candidate.Kind == "ipv6" && !addr.Addr().Is6()) {
			continue
		}
		candidate.Address = addr.String()
		validated = upsertCandidate(validated, candidate)
	}
	if session.UDPAddr != nil {
		validated = upsertCandidate(validated, protocol.Candidate{Kind: "server_reflexive", Address: session.UDPAddr.String(), Priority: 100})
	}
	session.Candidates = validated
	session.CandidateGeneration++
	session.Reports = make(map[uint64]protocol.NetworkReport)
	session.ReportTimes = make(map[uint64]time.Time)
	for _, peer := range s.sessions {
		delete(peer.Reports, id)
		delete(peer.ReportTimes, id)
	}
	return nil
}

func (s *State) PeerCandidates(id uint64) []protocol.PeerCandidates {
	s.mu.RLock()
	defer s.mu.RUnlock()
	self := s.sessions[id]
	if self == nil || self.ChannelID == 0 {
		return nil
	}
	result := make([]protocol.PeerCandidates, 0)
	for peerID, peer := range s.sessions {
		if peerID != id && peer.ChannelID == self.ChannelID {
			result = append(result, protocol.PeerCandidates{PeerID: peerID, Generation: peer.CandidateGeneration, Candidates: append([]protocol.Candidate(nil), peer.Candidates...), SecurityKey: peer.ClientEphemeralKey})
		}
	}
	return result
}

func (s *State) CandidateSnapshot(id uint64) (protocol.PeerCandidates, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	x := s.sessions[id]
	if x == nil {
		return protocol.PeerCandidates{}, false
	}
	return protocol.PeerCandidates{PeerID: id, Generation: x.CandidateGeneration, Candidates: append([]protocol.Candidate(nil), x.Candidates...), SecurityKey: x.ClientEphemeralKey}, true
}

func (s *State) SendTo(id uint64, envelope protocol.Envelope) bool {
	s.mu.RLock()
	session := s.sessions[id]
	var sender Sender
	if session != nil {
		sender = session.Sender
	}
	s.mu.RUnlock()
	if sender == nil {
		return false
	}
	return sender.Send(envelope) == nil
}

func (s *State) BroadcastRoom(channelID uint64, envelope protocol.Envelope, except uint64) {
	s.mu.RLock()
	senders := make([]Sender, 0)
	for id, session := range s.sessions {
		if id != except && session.ChannelID == channelID && session.Sender != nil {
			senders = append(senders, session.Sender)
		}
	}
	s.mu.RUnlock()
	for _, sender := range senders {
		_ = sender.Send(envelope)
	}
}

func (s *State) ChannelOf(id uint64) uint64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if x := s.sessions[id]; x != nil {
		return x.ChannelID
	}
	return 0
}

func (s *State) Channel(id uint64) (protocol.Channel, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	channel, ok := s.channels[id]
	return channel, ok
}
func (s *State) RoomSize(channelID uint64) int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.roomSizeLocked(channelID)
}

// PeerCandidateFanoutAllowed bounds the room's full candidate matrix to the
// configured direct-routing capacity. Small forced-SFU rooms keep receiving
// candidates for wire compatibility and diagnostics, while larger rooms avoid
// an O(n²) fanout that cannot affect their server-only route.
func (s *State) PeerCandidateFanoutAllowed(channelID uint64) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if channelID == 0 {
		return false
	}
	size := s.roomSizeLocked(channelID)
	return size >= 2 && size <= s.network.P2PMaxRoomSize
}

func (s *State) RoomSessionIDs(channelID uint64) []uint64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	ids := make([]uint64, 0, s.roomSizeLocked(channelID))
	for id, session := range s.sessions {
		if session.ChannelID == channelID {
			ids = append(ids, id)
		}
	}
	return ids
}

func (s *State) ActiveRooms() []uint64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	rooms := make(map[uint64]struct{})
	for _, session := range s.sessions {
		if session.ChannelID != 0 {
			rooms[session.ChannelID] = struct{}{}
		}
	}
	result := make([]uint64, 0, len(rooms))
	for roomID := range rooms {
		result = append(result, roomID)
	}
	sort.Slice(result, func(i, j int) bool { return result[i] < result[j] })
	return result
}

func (s *State) roomSizeLocked(channelID uint64) int {
	n := 0
	for _, x := range s.sessions {
		if x.ChannelID == channelID {
			n++
		}
	}
	return n
}

func (s *State) SetTransportMode(id uint64, mode string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	x := s.sessions[id]
	if x == nil {
		return ErrUnknownSession
	}
	x.TransportMode = mode
	return nil
}

func (s *State) SetRoomNetworkPolicy(id uint64, policy string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil {
		return ErrUnknownSession
	}
	if session.ChannelID == 0 {
		return errors.New("join a room before setting network policy")
	}
	return s.setRoomNetworkPolicyLocked(session.ChannelID, policy)
}

func (s *State) SetRoomNetworkPolicyForChannel(channelID uint64, policy string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.channels[channelID]; !exists {
		return ErrUnknownChannel
	}
	return s.setRoomNetworkPolicyLocked(channelID, policy)
}

func (s *State) setRoomNetworkPolicyLocked(channelID uint64, policy string) error {
	if policy != "auto" && policy != "force_sfu" && policy != "force_p2p" {
		return errors.New("invalid room network policy")
	}
	if policy == "force_p2p" {
		roomSize := s.roomSizeLocked(channelID)
		if !s.network.P2PEnabled || roomSize > s.network.P2PMaxRoomSize || roomSize > s.network.HybridMaxRoomSize {
			return ErrP2PNotSupported
		}
	}
	s.roomPolicy[channelID] = policy
	channel := s.channels[channelID]
	channel.NetworkPolicy = policy
	s.channels[channelID] = channel
	return nil
}

func (s *State) RoomNetworkPolicy(channelID uint64) string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	policy := s.roomPolicy[channelID]
	if policy == "" {
		return "auto"
	}
	return policy
}
func (s *State) SetNetworkReport(id uint64, report protocol.NetworkReport) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	x := s.sessions[id]
	if x == nil {
		return ErrUnknownSession
	}
	if report.PeerID == id {
		return errors.New("cannot report self")
	}
	peer := s.sessions[report.PeerID]
	if x.ChannelID == 0 || peer == nil || peer.ChannelID != x.ChannelID {
		return errors.New("network report target is outside room")
	}
	if !validNetworkMetric(report.RTTMS, 60_000) ||
		!validNetworkMetric(report.JitterMS, 60_000) ||
		!validNetworkMetric(report.Loss, 100) {
		return errors.New("invalid network report metrics")
	}
	x.Reports[report.PeerID] = report
	x.ReportTimes[report.PeerID] = time.Now()
	return nil
}

func validNetworkMetric(value, maximum float64) bool {
	return !math.IsNaN(value) && !math.IsInf(value, 0) && value >= 0 && value <= maximum
}
func (s *State) SetSubscriptions(id uint64, streamIDs []uint64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	x := s.sessions[id]
	if x == nil {
		return ErrUnknownSession
	}
	next := make(map[uint64]bool, len(streamIDs))
	for _, streamID := range streamIDs {
		peer := s.sessions[streamID]
		if peer == nil || peer.ChannelID == 0 || peer.ChannelID != x.ChannelID {
			return errors.New("subscription stream is outside room")
		}
		if streamID != id {
			next[streamID] = true
		}
	}
	x.Subscriptions = next
	return nil
}
func (s *State) NetworkConfig() config.Network { return s.network }
func (s *State) RuntimeStats() RuntimeStats {
	s.mu.RLock()
	defer s.mu.RUnlock()
	rooms := make(map[uint64]bool)
	stats := RuntimeStats{OnlineUsers: len(s.sessions)}
	for _, x := range s.sessions {
		if x.ChannelID != 0 {
			rooms[x.ChannelID] = true
		}
		if x.TransportMode == "force_relay" {
			stats.RelaySessions++
		}
	}
	stats.Rooms = len(rooms)
	for room := range rooms {
		if s.roomTopology[room].Topology == "p2p_stable" {
			stats.P2PSessions++
		} else {
			stats.SFUSessions++
		}
	}
	return stats
}

func (s *State) EvaluateRoom(channelID uint64) protocol.RouteUpdate {
	s.mu.Lock()
	defer s.mu.Unlock()
	cfg := s.network
	members := make([]*Session, 0)
	for _, x := range s.sessions {
		if x.ChannelID == channelID {
			members = append(members, x)
		}
	}
	policy := s.roomPolicy[channelID]
	if policy == "" {
		policy = "auto"
	}
	update := protocol.RouteUpdate{RoomID: channelID, Topology: "sfu_stable", Transport: "sfu", Reason: "sfu_first", RoomNetworkPolicy: policy, QualificationMS: cfg.QualificationDurationMS, MinimumDwellMS: cfg.MinimumDwellTimeMS, MigrationCooldownMS: cfg.MigrationCooldownMS, DegradeDurationMS: cfg.DegradeDurationMS, MaxRTTMS: cfg.QualificationRTTMS, MaxLossPercent: cfg.QualificationLossPercent, MaxJitterMS: cfg.QualificationJitterMS, DegradeRTTMS: cfg.RouteDegradeRTTMS, DegradeLossPercent: cfg.RouteDegradeLossPercent, DegradeJitterMS: cfg.RouteDegradeJitterMS}
	now := time.Now()
	previous := s.roomTopology[channelID]
	if policy == "force_sfu" {
		update.Reason = "forced_sfu"
		s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now, CooldownUntil: now.Add(time.Duration(cfg.MigrationCooldownMS) * time.Millisecond)}
		return update
	}
	for _, x := range members {
		if x.TransportMode == "force_relay" {
			if cfg.RelayEnabled {
				update.Transport = "relay"
				update.Reason = "forced_relay"
			} else {
				update.Reason = "relay_disabled"
			}
			s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now, CooldownUntil: now.Add(time.Duration(cfg.MigrationCooldownMS) * time.Millisecond)}
			return update
		}
		if x.TransportMode == "force_sfu" {
			update.Reason = "forced_sfu"
			s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now, CooldownUntil: now.Add(time.Duration(cfg.MigrationCooldownMS) * time.Millisecond)}
			return update
		}
	}
	if !cfg.P2PEnabled || len(members) < 2 || len(members) > cfg.HybridMaxRoomSize {
		if policy == "force_p2p" && (!cfg.P2PEnabled || len(members) > cfg.HybridMaxRoomSize) {
			update.Reason = "p2p_not_supported_for_current_room"
		} else {
			update.Reason = "p2p_ineligible"
		}
		// A one-member/new room has not degraded from P2P, so it must not
		// impose a migration cooldown on peers that subsequently join.
		s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now}
		return update
	}
	if len(members) > cfg.P2PMaxRoomSize {
		if policy == "force_p2p" {
			update.Reason = "p2p_not_supported_for_current_room"
		} else {
			update.Reason = "room_too_large_for_p2p"
		}
		s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now}
		return update
	}
	qualified := true
	explicitFailure := false
	for _, x := range members {
		for _, y := range members {
			if x.ID != y.ID {
				report, ok := x.Reports[y.ID]
				// Reports normally arrive every second. Missing control updates
				// must not preserve a historical P2P qualification indefinitely.
				if ok && now.Sub(x.ReportTimes[y.ID]) >= 3*time.Second {
					report.Qualified = false
				}
				if !ok || !report.Qualified {
					qualified = false
				}
				if ok && !report.Qualified {
					explicitFailure = true
				}
			}
		}
	}
	if qualified {
		minimumDwell := time.Duration(cfg.MinimumDwellTimeMS) * time.Millisecond
		if (previous.Topology != "transitional_hybrid" && now.Before(previous.CooldownUntil)) || (previous.Topology == "sfu_stable" && now.Sub(previous.LastChanged) < minimumDwell) {
			update.Topology = "p2p_evaluating"
			update.Reason = "migration_cooldown"
			return update
		}
		update.Topology = "p2p_stable"
		update.Transport = "p2p"
		if policy == "force_p2p" {
			update.Reason = "forced_p2p_qualified"
		} else {
			update.Reason = "full_room_qualified"
		}
		if previous.Topology != "p2p_stable" {
			s.roomTopology[channelID] = roomRoutingState{Topology: "p2p_stable", LastChanged: now}
		}
		return update
	}
	if previous.Topology == "p2p_stable" && !explicitFailure {
		timeout := time.Duration(cfg.QualificationDurationMS+cfg.DegradeDurationMS) * time.Millisecond
		s.roomTopology[channelID] = roomRoutingState{
			Topology:      "transitional_hybrid",
			LastChanged:   now,
			CooldownUntil: now.Add(timeout),
		}
		update.Topology = "transitional_hybrid"
		update.Transport = "hybrid"
		update.Reason = "membership_change_requalification"
		return update
	}
	if previous.Topology == "transitional_hybrid" && !explicitFailure {
		if now.Before(previous.CooldownUntil) {
			update.Topology = "transitional_hybrid"
			update.Transport = "hybrid"
			update.Reason = "membership_change_requalification"
			return update
		}
		s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now}
		update.Reason = "p2p_qualification_timeout"
		return update
	}
	if (previous.Topology == "p2p_stable" || previous.Topology == "transitional_hybrid") && explicitFailure {
		recoveryTopology := "sfu_stable"
		if policy == "force_p2p" {
			recoveryTopology = "p2p_recovering"
		}
		s.roomTopology[channelID] = roomRoutingState{Topology: recoveryTopology, LastChanged: now, CooldownUntil: now.Add(time.Duration(cfg.MigrationCooldownMS) * time.Millisecond)}
		update.Topology = "migrating_to_sfu"
		if policy == "force_p2p" {
			update.Reason = "temporary_sfu_recovery"
		} else {
			update.Reason = "p2p_degraded"
		}
		return update
	}
	if previous.Topology == "" {
		s.roomTopology[channelID] = roomRoutingState{Topology: "sfu_stable", LastChanged: now}
	}
	update.Topology = "p2p_evaluating"
	if policy == "force_p2p" {
		update.Topology = "p2p_pending"
		update.Reason = "forced_p2p_pending"
	} else {
		update.Reason = "awaiting_full_room_qualification"
	}
	return update
}

func upsertCandidate(values []protocol.Candidate, candidate protocol.Candidate) []protocol.Candidate {
	for i := range values {
		if values[i].Address == candidate.Address {
			values[i] = candidate
			return values
		}
	}
	return append(values, candidate)
}

type UDPSourceValidation uint8

const (
	UDPSourceValid UDPSourceValidation = iota
	UDPSourceUnknownSession
	UDPSourceUnregistered
	UDPSourceMismatch
)

// CheckUDPSource is allocation-free; packet routing does not need a session copy.
func (s *State) CheckUDPSource(id uint64, addr *net.UDPAddr) UDPSourceValidation {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return validateUDPSource(s.sessions[id], addr)
}
func validateUDPSource(session *Session, addr *net.UDPAddr) UDPSourceValidation {
	if session == nil {
		return UDPSourceUnknownSession
	}
	if session.UDPAddr == nil {
		return UDPSourceUnregistered
	}
	if addr == nil || !session.UDPAddr.IP.Equal(addr.IP) || session.UDPAddr.Port != addr.Port {
		return UDPSourceMismatch
	}
	return UDPSourceValid
}

func (s *State) ValidateUDPSource(id uint64, addr *net.UDPAddr) (*Session, UDPSourceValidation) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	if result := validateUDPSource(session, addr); result != UDPSourceValid {
		return nil, result
	}
	return cloneSession(session), UDPSourceValid
}

func (s *State) ValidateUDP(id uint64, addr *net.UDPAddr) (*Session, bool) {
	session, validation := s.ValidateUDPSource(id, addr)
	return session, validation == UDPSourceValid
}

func (s *State) VoiceRecipients(senderID uint64) []*net.UDPAddr {
	targets := s.VoiceTargets(senderID)
	result := make([]*net.UDPAddr, 0, len(targets))
	for _, target := range targets {
		result = append(result, target.Address)
	}
	return result
}

type VoiceTarget struct {
	SessionID     uint64
	Address       *net.UDPAddr
	MediaOutbound *security.MediaCipher
}

func (s *State) VoiceTargets(senderID uint64) []VoiceTarget {
	s.mu.RLock()
	defer s.mu.RUnlock()
	sender := s.sessions[senderID]
	if sender == nil || sender.ChannelID == 0 {
		return nil
	}
	members := s.roomMembers[sender.ChannelID]
	result := make([]VoiceTarget, 0, max(0, len(members)-1))
	for id, session := range members {
		if id != senderID && session.ChannelID == sender.ChannelID && session.UDPAddr != nil && !session.Deafened && (len(session.Subscriptions) == 0 || session.Subscriptions[senderID]) {
			result = append(result, VoiceTarget{SessionID: id, Address: cloneUDPAddr(session.UDPAddr), MediaOutbound: session.MediaOutbound})
		}
	}
	return result
}

func (s *State) CanSpeak(id uint64) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	return session != nil && !session.Muted && !effectiveAdminMuted(session)
}

func (s *State) Snapshot() protocol.ServerState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	channels := make([]protocol.Channel, 0, len(s.channels))
	for _, channel := range s.channels {
		channels = append(channels, channel)
	}
	sort.Slice(channels, func(i, j int) bool {
		if channels[i].SortOrder == channels[j].SortOrder {
			return channels[i].ID < channels[j].ID
		}
		return channels[i].SortOrder < channels[j].SortOrder
	})
	users := make([]protocol.User, 0, len(s.sessions))
	for _, session := range s.sessions {
		users = append(users, userFromSession(session))
	}
	sort.Slice(users, func(i, j int) bool { return users[i].SessionID < users[j].SessionID })
	scenes := map[uint64]protocol.RoomScene{}
	for id, d := range s.scenes {
		if d.current.GameID != "" {
			scenes[id] = d.current
		}
	}
	return protocol.ServerState{ServerName: s.serverName, Channels: channels, Users: users, GameCatalogVersion: s.gameCatalog.Version, RoomScenes: scenes}
}

func (s *State) User(id uint64) (protocol.User, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session, ok := s.sessions[id]
	if !ok {
		return protocol.User{}, false
	}
	return userFromSession(session), true
}

func (s *State) Broadcast(envelope protocol.Envelope, except uint64) {
	s.mu.RLock()
	senders := make([]Sender, 0, len(s.sessions))
	for id, session := range s.sessions {
		if id != except && session.Sender != nil {
			senders = append(senders, session.Sender)
		}
	}
	s.mu.RUnlock()
	for _, sender := range senders {
		_ = sender.Send(envelope)
	}
}

func (s *State) Expired(before time.Time) []uint64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var ids []uint64
	for id, session := range s.sessions {
		if session.LastHeartbeat.Before(before) {
			ids = append(ids, id)
		}
	}
	return ids
}

func userFromSession(session *Session) protocol.User {
	channelMuted := session.ChannelID != 0 && session.ChannelMutedIn == session.ChannelID
	adminMuted := session.AdminMuted || channelMuted
	return protocol.User{GameActivity: session.GameActivity, SessionID: session.ID, Nickname: session.Nickname, ChannelID: session.ChannelID, Muted: session.Muted || adminMuted, Deafened: session.Deafened, ClientID: session.ClientID, ServerRole: session.ServerRole, Avatar: session.Avatar, AdminMuted: adminMuted, ServerMuted: session.AdminMuted, ChannelMuted: channelMuted}
}
func effectiveAdminMuted(session *Session) bool {
	return session.AdminMuted || session.ChannelID != 0 && session.ChannelMutedIn == session.ChannelID
}
func cloneSession(session *Session) *Session {
	if session == nil {
		return nil
	}
	result := *session
	result.UDPAddr = cloneUDPAddr(session.UDPAddr)
	result.Candidates = append([]protocol.Candidate(nil), session.Candidates...)
	result.Reports = make(map[uint64]protocol.NetworkReport, len(session.Reports))
	for k, v := range session.Reports {
		result.Reports[k] = v
	}
	result.ReportTimes = make(map[uint64]time.Time, len(session.ReportTimes))
	for k, v := range session.ReportTimes {
		result.ReportTimes[k] = v
	}
	result.Subscriptions = make(map[uint64]bool, len(session.Subscriptions))
	for k, v := range session.Subscriptions {
		result.Subscriptions[k] = v
	}
	return &result
}
func cloneUDPAddr(addr *net.UDPAddr) *net.UDPAddr {
	if addr == nil {
		return nil
	}
	return &net.UDPAddr{IP: append(net.IP(nil), addr.IP...), Port: addr.Port, Zone: addr.Zone}
}
