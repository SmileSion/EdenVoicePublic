package administration

import (
	cryptorand "crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"sort"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/privatefs"
	"edenvoice/server/internal/protocol"
)

type ServerRole string
type ChannelRole string
type Action string

const (
	RoleMaster ServerRole = "MASTER"
	RoleAdmin  ServerRole = "ADMIN"
	RoleUser   ServerRole = "USER"

	RoleChannelAdmin ChannelRole = "CHANNEL_ADMIN"
	RoleChannelUser  ChannelRole = "CHANNEL_USER"

	ActionChannelCreate     Action = "channel.create"
	ActionChannelEdit       Action = "channel.edit"
	ActionChannelDelete     Action = "channel.delete"
	ActionChannelRoleSet    Action = "channel.role.set"
	ActionNetworkPolicySet  Action = "network.policy.set"
	ActionMemberMute        Action = "member.mute"
	ActionMemberMove        Action = "member.move"
	ActionMemberKickChannel Action = "member.kick_channel"
	ActionMemberKickServer  Action = "member.kick_server"
	ActionMemberBan         Action = "member.ban"
	ActionMemberUnban       Action = "member.unban"
	ActionMemberBanList     Action = "member.ban_list"
	ActionServerRoleSet     Action = "server.role.set"
	ActionWatchManage       Action = "watch.manage"
)

var (
	ErrForbidden                = errors.New("permission denied")
	ErrBanned                   = errors.New("identity is banned")
	ErrMasterAlreadyProvisioned = errors.New("master is already provisioned")
)

type User struct {
	ClientID    string     `json:"client_id"`
	PublicKey   string     `json:"public_key"`
	DisplayName string     `json:"display_name"`
	Avatar      string     `json:"avatar,omitempty"`
	AvatarData  string     `json:"avatar_data,omitempty"`
	ServerRole  ServerRole `json:"server_role"`
	FirstSeen   time.Time  `json:"first_seen"`
	LastSeen    time.Time  `json:"last_seen"`
}

type Ban struct {
	ClientID         string     `json:"client_id"`
	Reason           string     `json:"reason"`
	CreatedAt        time.Time  `json:"created_at"`
	ExpiresAt        *time.Time `json:"expires_at,omitempty"`
	OperatorClientID string     `json:"operator_client_id"`
}

type persistentState struct {
	ChannelPasswords map[uint64]string                 `json:"channel_passwords,omitempty"`
	LastChannelID    uint64                            `json:"last_channel_id"`
	Version          int                               `json:"version"`
	Users            map[string]User                   `json:"users"`
	ChannelRoles     map[uint64]map[string]ChannelRole `json:"channel_roles"`
	Bans             map[string]Ban                    `json:"bans"`
	Channels         map[uint64]protocol.Channel       `json:"channels"`
	MasterTokenHash  string                            `json:"master_token_hash,omitempty"`
}

type Store struct {
	mu    sync.RWMutex
	path  string
	state persistentState
}

func LoadOrCreate(path string, initialChannels []protocol.Channel) (*Store, string, error) {
	return LoadOrCreateWithToken(path, initialChannels, "")
}

func LoadOrCreateWithToken(path string, initialChannels []protocol.Channel, configuredToken string) (*Store, string, error) {
	store := &Store{path: path}
	bytes, err := os.ReadFile(path)
	if err == nil {
		if err := privatefs.Ensure(path); err != nil {
			return nil, "", err
		}
		if err := json.Unmarshal(bytes, &store.state); err != nil {
			return nil, "", fmt.Errorf("decode administration state: %w", err)
		}
		store.normalize()
		token, err := store.rotateProvisionTokenIfNeeded(configuredToken)
		return store, token, err
	}
	if !errors.Is(err, os.ErrNotExist) {
		return nil, "", err
	}
	store.state = persistentState{Version: 1, Users: make(map[string]User), ChannelRoles: make(map[uint64]map[string]ChannelRole), Bans: make(map[string]Ban), Channels: make(map[uint64]protocol.Channel)}
	for _, channel := range initialChannels {
		if channel.NetworkPolicy == "" {
			channel.NetworkPolicy = "auto"
		}
		if channel.AudioQualityPolicy == "" {
			channel.AudioQualityPolicy = "auto"
		}
		channel.Temporary = false
		store.state.Channels[channel.ID] = channel
		store.state.LastChannelID = max(store.state.LastChannelID, channel.ID)
	}
	token := configuredToken
	generated := token == ""
	if generated {
		token, err = randomToken()
		if err != nil {
			return nil, "", err
		}
	}
	digest := sha256.Sum256([]byte(token))
	store.state.MasterTokenHash = base64.RawStdEncoding.EncodeToString(digest[:])
	if err := store.persistLocked(); err != nil {
		return nil, "", err
	}
	if !generated {
		return store, "", nil
	}
	return store, token, nil
}

func (s *Store) rotateProvisionTokenIfNeeded(configuredToken string) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, user := range s.state.Users {
		if user.ServerRole == RoleMaster {
			return "", nil
		}
	}
	token := configuredToken
	generated := token == ""
	var err error
	if generated {
		token, err = randomToken()
		if err != nil {
			return "", err
		}
	}
	digest := sha256.Sum256([]byte(token))
	s.state.MasterTokenHash = base64.RawStdEncoding.EncodeToString(digest[:])
	if err := s.persistLocked(); err != nil {
		return "", err
	}
	if generated {
		return token, nil
	}
	return "", nil
}

func (s *Store) normalize() {
	if s.state.Users == nil {
		s.state.Users = make(map[string]User)
	}
	if s.state.ChannelRoles == nil {
		s.state.ChannelRoles = make(map[uint64]map[string]ChannelRole)
	}
	if s.state.Bans == nil {
		s.state.Bans = make(map[string]Ban)
	}
	if s.state.Channels == nil {
		s.state.Channels = make(map[uint64]protocol.Channel)
	}
	for id, channel := range s.state.Channels {
		s.state.LastChannelID = max(s.state.LastChannelID, id)
		changed := false
		if channel.NetworkPolicy == "" {
			channel.NetworkPolicy = "auto"
			changed = true
		}
		if channel.AudioQualityPolicy == "" {
			channel.AudioQualityPolicy = "auto"
			changed = true
		}
		if changed {
			s.state.Channels[id] = channel
		}
	}
	for id, user := range s.state.Users {
		reference, data, err := protocol.CanonicalAvatar(user.Avatar)
		if err != nil {
			user.Avatar = ""
			user.AvatarData = ""
			s.state.Users[id] = user
		} else if data != "" {
			user.Avatar = reference
			user.AvatarData = data
			s.state.Users[id] = user
		} else if strings.HasPrefix(reference, protocol.CustomAvatarRefPrefix) {
			storedReference, storedData, storedErr := protocol.CanonicalAvatar(user.AvatarData)
			if storedErr != nil || storedData == "" || storedReference != reference {
				user.Avatar = ""
				user.AvatarData = ""
				s.state.Users[id] = user
			}
		}
	}
}

func (s *Store) Admit(clientID, publicKey, displayName string, now time.Time) (User, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if ban, ok := s.state.Bans[clientID]; ok {
		if ban.ExpiresAt == nil || ban.ExpiresAt.After(now) {
			return User{}, ErrBanned
		}
		delete(s.state.Bans, clientID)
	}
	user, exists := s.state.Users[clientID]
	if exists && user.PublicKey != publicKey {
		return User{}, errors.New("stored identity public key mismatch")
	}
	if !exists {
		user = User{ClientID: clientID, PublicKey: publicKey, ServerRole: RoleUser, FirstSeen: now}
	}
	user.DisplayName = displayName
	user.LastSeen = now
	s.state.Users[clientID] = user
	return user, s.persistLocked()
}

func (s *Store) UpdateDisplayName(clientID, displayName string, now time.Time) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.state.Users[clientID]
	if !ok {
		return errors.New("unknown client identity")
	}
	user.DisplayName = displayName
	user.LastSeen = now
	s.state.Users[clientID] = user
	return s.persistLocked()
}

func (s *Store) UpdateAvatar(clientID, avatar string, now time.Time) error {
	reference, data, err := protocol.CanonicalAvatar(avatar)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.state.Users[clientID]
	if !ok {
		return errors.New("unknown client identity")
	}
	user.Avatar = reference
	if data != "" {
		user.AvatarData = data
	} else if !strings.HasPrefix(reference, protocol.CustomAvatarRefPrefix) {
		user.AvatarData = ""
	}
	user.LastSeen = now
	s.state.Users[clientID] = user
	return s.persistLocked()
}

func (s *Store) AvatarData(reference string) (string, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, user := range s.state.Users {
		if user.Avatar == reference && user.AvatarData != "" {
			return user.AvatarData, true
		}
	}
	return "", false
}

type MuteScope uint8

const (
	MuteScopeServer MuteScope = iota + 1
	MuteScopeChannel
)

// AuthorizeMute keeps role-to-scope selection inside the authorization policy.
func (s *Store) AuthorizeMute(subject string, resource Resource) (MuteScope, error) {
	if err := s.Authorize(subject, ActionMemberMute, resource); err != nil {
		return 0, err
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	actor := s.state.Users[subject]
	if actor.ServerRole == RoleMaster || actor.ServerRole == RoleAdmin {
		return MuteScopeServer, nil
	}
	return MuteScopeChannel, nil
}

func (s *Store) ProvisionMaster(clientID, token string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, user := range s.state.Users {
		if user.ServerRole == RoleMaster {
			return ErrMasterAlreadyProvisioned
		}
	}
	provided := sha256.Sum256([]byte(token))
	expected, err := base64.RawStdEncoding.DecodeString(s.state.MasterTokenHash)
	if err != nil || len(expected) != len(provided) || subtle.ConstantTimeCompare(expected, provided[:]) != 1 {
		return ErrForbidden
	}
	user, ok := s.state.Users[clientID]
	if !ok {
		return errors.New("unknown client identity")
	}
	user.ServerRole = RoleMaster
	s.state.Users[clientID] = user
	s.state.MasterTokenHash = ""
	return s.persistLocked()
}

func (s *Store) RecoverMaster(clientID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.state.Users[clientID]
	if !ok {
		return errors.New("unknown client identity")
	}
	for id, existing := range s.state.Users {
		if existing.ServerRole == RoleMaster {
			existing.ServerRole = RoleUser
			s.state.Users[id] = existing
		}
	}
	user.ServerRole = RoleMaster
	s.state.Users[clientID] = user
	s.state.MasterTokenHash = ""
	return s.persistLocked()
}

type Resource struct {
	ChannelID      uint64
	TargetClientID string
}

func (s *Store) Authorize(subject string, action Action, resource Resource) error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	actor, ok := s.state.Users[subject]
	if !ok {
		return ErrForbidden
	}
	if actor.ServerRole == RoleMaster {
		return nil
	}
	target := s.state.Users[resource.TargetClientID]
	if actor.ServerRole == RoleAdmin {
		if target.ServerRole == RoleMaster || action == ActionServerRoleSet {
			return ErrForbidden
		}
		return nil
	}
	channelRole := s.state.ChannelRoles[resource.ChannelID][subject]
	if channelRole != RoleChannelAdmin {
		return ErrForbidden
	}
	if target.ServerRole == RoleMaster || target.ServerRole == RoleAdmin {
		return ErrForbidden
	}
	switch action {
	case ActionChannelEdit, ActionNetworkPolicySet, ActionMemberMute, ActionMemberMove, ActionMemberKickChannel, ActionWatchManage:
		return nil
	default:
		return ErrForbidden
	}
}

func (s *Store) SetServerRole(target string, role ServerRole) error {
	if role != RoleAdmin && role != RoleUser {
		return errors.New("only ADMIN or USER can be assigned")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.state.Users[target]
	if !ok {
		return errors.New("unknown client identity")
	}
	if user.ServerRole == RoleMaster {
		return ErrForbidden
	}
	user.ServerRole = role
	s.state.Users[target] = user
	return s.persistLocked()
}

func (s *Store) SetChannelRole(channelID uint64, clientID string, role ChannelRole) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.state.Channels[channelID]; !ok {
		return errors.New("unknown channel")
	}
	if _, ok := s.state.Users[clientID]; !ok {
		return errors.New("unknown client identity")
	}
	roles := s.state.ChannelRoles[channelID]
	if roles == nil {
		roles = make(map[string]ChannelRole)
		s.state.ChannelRoles[channelID] = roles
	}
	if role == "" || role == RoleChannelUser {
		delete(roles, clientID)
	} else if role == RoleChannelAdmin {
		roles[clientID] = role
	} else {
		return errors.New("invalid channel role")
	}
	return s.persistLocked()
}

func (s *Store) CreateChannel(channel protocol.Channel) (protocol.Channel, error) {
	return s.CreateChannelWithPassword(channel, "")
}
func (s *Store) CreateChannelWithPassword(channel protocol.Channel, password string) (protocol.Channel, error) {
	hash, err := hashChannelPassword(password)
	if err != nil {
		return protocol.Channel{}, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if channel.Name == "" {
		return protocol.Channel{}, errors.New("invalid channel")
	}
	channel.NetworkPolicy = "auto"
	if channel.AudioQualityPolicy == "" {
		channel.AudioQualityPolicy = "auto"
	}
	if !validChannelAudioQualityPolicy(channel.AudioQualityPolicy) {
		return protocol.Channel{}, errors.New("invalid channel audio quality policy")
	}
	if channel.ID == 0 {
		if s.state.LastChannelID == math.MaxUint64 {
			return protocol.Channel{}, errors.New("channel IDs exhausted")
		}
		channel.ID = s.state.LastChannelID + 1
	}
	if _, exists := s.state.Channels[channel.ID]; exists {
		return protocol.Channel{}, errors.New("channel already exists")
	}
	if channel.ID <= s.state.LastChannelID {
		return protocol.Channel{}, errors.New("channel ID cannot be reused")
	}
	if channel.ParentID != 0 {
		parent, ok := s.state.Channels[channel.ParentID]
		if !ok {
			return protocol.Channel{}, errors.New("unknown parent channel")
		}
		if parent.Temporary {
			return protocol.Channel{}, errors.New("temporary channels cannot have sub-channels")
		}
	}
	// Persist the high-water mark even for temporary rooms, whose definitions
	// are omitted from disk. Their retained chat/archive must never be inherited.
	s.state.LastChannelID = channel.ID
	s.state.Channels[channel.ID] = channel
	if hash != "" {
		if s.state.ChannelPasswords == nil {
			s.state.ChannelPasswords = make(map[uint64]string)
		}
		s.state.ChannelPasswords[channel.ID] = hash
	}
	if err := s.persistLocked(); err != nil {
		delete(s.state.Channels, channel.ID)
		delete(s.state.ChannelPasswords, channel.ID)
		return protocol.Channel{}, err
	}
	return channel, nil
}

func (s *Store) EditChannel(channel protocol.Channel) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	existing, exists := s.state.Channels[channel.ID]
	if !exists {
		return errors.New("unknown channel")
	}
	if channel.Name == "" || channel.ParentID == channel.ID {
		return errors.New("invalid channel")
	}
	if channel.ParentID != 0 {
		parent, exists := s.state.Channels[channel.ParentID]
		if !exists {
			return errors.New("unknown parent channel")
		}
		if parent.Temporary {
			return errors.New("temporary channels cannot have sub-channels")
		}
		for parentID := channel.ParentID; parentID != 0; {
			if parentID == channel.ID {
				return errors.New("channel hierarchy cycle")
			}
			parent, exists := s.state.Channels[parentID]
			if !exists {
				return errors.New("invalid parent channel hierarchy")
			}
			parentID = parent.ParentID
		}
	}
	if channel.Temporary {
		for _, candidate := range s.state.Channels {
			if candidate.ParentID == channel.ID {
				return errors.New("a channel with sub-channels cannot be temporary")
			}
		}
	}
	if channel.AudioQualityPolicy == "" {
		channel.AudioQualityPolicy = existing.AudioQualityPolicy
	}
	if !channel.TemporarySpecified {
		channel.Temporary = existing.Temporary
	}
	if !validChannelAudioQualityPolicy(channel.AudioQualityPolicy) {
		return errors.New("invalid channel audio quality policy")
	}
	channel.NetworkPolicy = existing.NetworkPolicy
	s.state.Channels[channel.ID] = channel
	return s.persistLocked()
}

func (s *Store) SetChannelNetworkPolicy(channelID uint64, policy string) error {
	if policy != "auto" && policy != "force_sfu" && policy != "force_p2p" {
		return errors.New("invalid room network policy")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	channel, exists := s.state.Channels[channelID]
	if !exists {
		return errors.New("unknown channel")
	}
	channel.NetworkPolicy = policy
	s.state.Channels[channelID] = channel
	return s.persistLocked()
}

func (s *Store) DeleteChannel(channelID uint64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.state.Channels[channelID]; !exists {
		return errors.New("unknown channel")
	}
	for _, channel := range s.state.Channels {
		if channel.ParentID == channelID {
			return errors.New("channel has sub-channels")
		}
	}
	delete(s.state.Channels, channelID)
	delete(s.state.ChannelRoles, channelID)
	return s.persistLocked()
}

func (s *Store) Ban(target, reason, operator string, expiresAt *time.Time) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.state.Bans[target] = Ban{ClientID: target, Reason: reason, CreatedAt: time.Now(), ExpiresAt: expiresAt, OperatorClientID: operator}
	return s.persistLocked()
}
func (s *Store) Unban(target string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.state.Bans, target)
	return s.persistLocked()
}
func (s *Store) Bans(now time.Time) []Ban {
	s.mu.RLock()
	defer s.mu.RUnlock()
	result := make([]Ban, 0, len(s.state.Bans))
	for _, ban := range s.state.Bans {
		if ban.ExpiresAt == nil || ban.ExpiresAt.After(now) {
			result = append(result, ban)
		}
	}
	sort.Slice(result, func(i, j int) bool {
		if result[i].CreatedAt.Equal(result[j].CreatedAt) {
			return result[i].ClientID < result[j].ClientID
		}
		return result[i].CreatedAt.After(result[j].CreatedAt)
	})
	return result
}
func (s *Store) User(clientID string) (User, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	user, ok := s.state.Users[clientID]
	return user, ok
}
func (s *Store) ChannelRole(channelID uint64, clientID string) ChannelRole {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.state.ChannelRoles[channelID][clientID]
}
func (s *Store) Channels() []protocol.Channel {
	s.mu.RLock()
	defer s.mu.RUnlock()
	result := make([]protocol.Channel, 0, len(s.state.Channels))
	for _, value := range s.state.Channels {
		result = append(result, value)
	}
	sort.Slice(result, func(i, j int) bool {
		return result[i].SortOrder < result[j].SortOrder || result[i].SortOrder == result[j].SortOrder && result[i].ID < result[j].ID
	})
	return result
}

func (s *Store) persistLocked() error {
	if err := privatefs.PrepareDirectory(s.path); err != nil {
		return err
	}
	persisted := s.state
	persisted.ChannelPasswords = make(map[uint64]string)
	for id, hash := range s.state.ChannelPasswords {
		if c, ok := s.state.Channels[id]; ok && !c.Temporary {
			persisted.ChannelPasswords[id] = hash
		}
	}
	persisted.Channels = make(map[uint64]protocol.Channel, len(s.state.Channels))
	persisted.ChannelRoles = make(map[uint64]map[string]ChannelRole, len(s.state.ChannelRoles))
	for id, channel := range s.state.Channels {
		if !channel.Temporary {
			persisted.Channels[id] = channel
		}
	}
	for id, roles := range s.state.ChannelRoles {
		if channel, exists := s.state.Channels[id]; exists && !channel.Temporary {
			persisted.ChannelRoles[id] = roles
		}
	}
	bytes, err := json.MarshalIndent(persisted, "", "  ")
	if err != nil {
		return err
	}
	temporary := s.path + ".tmp"
	if err := os.WriteFile(temporary, bytes, 0o600); err != nil {
		return err
	}
	if err := os.Rename(temporary, s.path); err != nil {
		_ = os.Remove(temporary)
		return err
	}
	return privatefs.Ensure(s.path)
}

func validChannelAudioQualityPolicy(policy string) bool {
	switch policy {
	case "auto",
		"quality_1", "quality_2", "quality_3", "quality_4", "quality_5",
		"quality_6", "quality_7", "quality_8", "quality_9", "quality_10",
		"fixed_32k", "fixed_48k", "fixed_64k", "fixed_96k":
		return true
	default:
		return false
	}
}

func randomToken() (string, error) {
	var bytes [24]byte
	if _, err := cryptorand.Read(bytes[:]); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(bytes[:]), nil
}

// Profiles excludes public keys and encoded images from administration listings.
func (s *Store) Profiles() []User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	users := make([]User, 0, len(s.state.Users))
	for _, u := range s.state.Users {
		u.PublicKey = ""
		u.AvatarData = ""
		users = append(users, u)
	}
	sort.Slice(users, func(i, j int) bool { return users[i].ClientID < users[j].ClientID })
	return users
}
