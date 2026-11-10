package control

import (
	"crypto/ecdh"
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
)

// The Rust client also decodes this fixture. Check actual sender JSON: a
// Go-only round trip would incorrectly accept null collections.
func TestArchiveWireCollectionsMatchSharedClientFixture(t *testing.T) {
	fixture, err := os.ReadFile("testdata/room-archive-empty.json")
	if err != nil {
		t.Fatal(err)
	}
	var envelope protocol.Envelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatal(err)
	}
	var snapshot archive.RoomSnapshot
	if err := envelope.Decode(&snapshot); err != nil {
		t.Fatal(err)
	}
	s := New("", "", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	if err := s.sendBoundedArchive(sender, snapshot); err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(sender.messages[0])
	if err != nil {
		t.Fatal(err)
	}
	var actual, expected map[string]json.RawMessage
	if err := json.Unmarshal(data, &actual); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(fixture, &expected); err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{"type", "enabled", "events", "active_day_count", "historical_member_count"} {
		if string(actual[field]) != string(expected[field]) {
			t.Fatalf("%s: got %s, want %s", field, actual[field], expected[field])
		}
	}
	var stats map[string]json.RawMessage
	if err := json.Unmarshal(actual["stats"], &stats); err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{"active_days", "active_hours", "members", "pairs"} {
		if string(stats[field]) != "{}" {
			t.Fatalf("%s must be {}, got %s", field, stats[field])
		}
	}
}

func TestCustomArchiveResponseRespectsControlFrameLimit(t *testing.T) {
	s := New("", "", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	snapshot := archive.RoomSnapshot{Enabled: true, Stats: archive.RoomStats{ActiveDays: map[string]bool{"2026-09-04": true}, Members: map[string]archive.MemberStats{"a": {}}}}
	for i := 0; i < 100; i++ {
		snapshot.Events = append(snapshot.Events, archive.Event{EventType: "custom", Titles: map[string]string{"zh-CN": strings.Repeat("声", 170)}, Descriptions: map[string]string{"zh-CN": strings.Repeat("音", 170)}})
	}
	if err := s.sendBoundedArchive(sender, snapshot); err != nil {
		t.Fatal(err)
	}
	if len(sender.messages) != 1 {
		t.Fatal("response not sent")
	}
	encoded, err := json.Marshal(sender.messages[0])
	if err != nil {
		t.Fatal(err)
	}
	if len(encoded)+1 > protocol.MaxControlMessageBytes {
		t.Fatalf("oversized archive response: %d", len(encoded))
	}
	var response archive.RoomSnapshot
	if err := sender.messages[0].Decode(&response); err != nil {
		t.Fatal(err)
	}
	if !response.HasMore || len(response.Events) == 0 || len(response.Events) >= 100 {
		t.Fatal("expected bounded recent events and has_more")
	}
	if response.ActiveDayCount != 1 || response.HistoricalMemberCount != 1 || len(response.Stats.Members) != 0 {
		t.Fatal("summary counts lost or private aggregation maps sent")
	}
}

func TestArchiveProfilesResolveNamesAndAvatarsWithoutPersistingThemInEvents(t *testing.T) {
	state := serverstate.New(config.Default())
	session := state.AddAuthenticatedSession("Eden", "client-a", "", "", nil, nil, nil)
	if err := state.SetAvatar(session.ID, "E"); err != nil {
		t.Fatal(err)
	}
	admin, _, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "administration.json"), nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := admin.Admit("client-b", "public-key", "Lumi", time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := admin.UpdateAvatar("client-b", "L", time.Now()); err != nil {
		t.Fatal(err)
	}
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = admin
	events := []archive.Event{{TriggeredBy: []string{"client-a"}, Participants: []string{"client-a", "client-b"}}}
	profiles := server.archiveParticipantProfiles(events)
	if profiles["client-a"].DisplayName != "Eden" || profiles["client-a"].Avatar != "E" {
		t.Fatalf("live profile = %#v", profiles["client-a"])
	}
	if profiles["client-b"].DisplayName != "Lumi" || profiles["client-b"].Avatar != "L" {
		t.Fatalf("historical profile = %#v", profiles["client-b"])
	}
	if events[0].TriggeredBy[0] != "client-a" {
		t.Fatalf("event identity changed: %#v", events[0])
	}
}

func TestArchiveFeatureIsNotAdvertisedWithoutCollector(t *testing.T) {
	server := New("", "", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	if features := server.serverFeatures(); !slices.Contains(features, "watch_party_v1") {
		t.Fatalf("watch party feature not advertised: %v", features)
	}
	if features := server.serverFeatures(); slices.Contains(features, "eden_archive_v1") {
		t.Fatalf("archive hard-off advertised features: %v", features)
	}

	collector, err := archive.LoadOrCreate(filepath.Join(t.TempDir(), "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = collector.Close() })
	server.archive = collector
	if features := server.serverFeatures(); !slices.Contains(features, "eden_archive_v1") {
		t.Fatalf("enabled archive did not advertise feature: %v", features)
	}
}

type testClient struct {
	conn      net.Conn
	enc       *json.Encoder
	dec       *json.Decoder
	clientID  string
	sessionID uint64
}

func TestSecureControlHandshakeAuthenticatesIdentityAndInstallsMediaKeys(t *testing.T) {
	identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "server-identity.json"))
	if err != nil {
		t.Fatal(err)
	}
	tlsConfig, err := identity.TLSConfig("Test")
	if err != nil {
		t.Fatal(err)
	}
	state := serverstate.New(config.Default())
	server := NewSecure("", "127.0.0.1:9001", state, slog.New(slog.NewTextHandler(io.Discard, nil)), identity, tlsConfig, false, nil)
	serverSide, clientSide := net.Pipe()
	go server.handle(tls.Server(serverSide, tlsConfig))
	clientTLS := tls.Client(clientSide, &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}) // test verifies EdenVoice identity below
	defer clientTLS.Close()
	encoder, decoder := json.NewEncoder(clientTLS), json.NewDecoder(clientTLS)
	clientPublic, clientPrivate, _ := ed25519.GenerateKey(cryptorand.Reader)
	clientEphemeral, _ := ecdh.X25519().GenerateKey(cryptorand.Reader)
	hello, _ := protocol.NewEnvelope(protocol.TypeHello, protocol.Hello{
		Nickname: "Alice", ProtocolVersion: security.ProtocolVersion,
		SecurityCapabilities: requiredSecurityCapabilities(), ClientID: security.ClientID(clientPublic),
		ClientPublicKey: base64.RawStdEncoding.EncodeToString(clientPublic), ClientEphemeralKey: base64.RawStdEncoding.EncodeToString(clientEphemeral.PublicKey().Bytes()),
	})
	if err := encoder.Encode(hello); err != nil {
		t.Fatal(err)
	}
	var challengeEnvelope protocol.Envelope
	if err := decoder.Decode(&challengeEnvelope); err != nil {
		t.Fatal(err)
	}
	if challengeEnvelope.Type != protocol.TypeIdentityChallenge {
		t.Fatalf("expected challenge, got %s", challengeEnvelope.Type)
	}
	var challenge protocol.IdentityChallenge
	if err := challengeEnvelope.Decode(&challenge); err != nil {
		t.Fatal(err)
	}
	challengeBytes, _ := base64.RawStdEncoding.DecodeString(challenge.Challenge)
	serverPublic, _ := base64.RawStdEncoding.DecodeString(challenge.ServerPublicKey)
	serverEphemeral, _ := base64.RawStdEncoding.DecodeString(challenge.ServerEphemeralKey)
	transcript := security.AuthTranscript(challengeBytes, serverPublic, clientEphemeral.PublicKey().Bytes(), serverEphemeral)
	response, _ := protocol.NewEnvelope(protocol.TypeIdentityResponse, protocol.IdentityResponse{Signature: base64.RawStdEncoding.EncodeToString(ed25519.Sign(clientPrivate, transcript))})
	if err := encoder.Encode(response); err != nil {
		t.Fatal(err)
	}
	var welcomeEnvelope protocol.Envelope
	if err := decoder.Decode(&welcomeEnvelope); err != nil {
		t.Fatal(err)
	}
	var welcome protocol.Welcome
	if err := welcomeEnvelope.Decode(&welcome); err != nil {
		t.Fatal(err)
	}
	if welcome.ClientID != security.ClientID(clientPublic) || welcome.MediaKeyEpoch != security.InitialKeyEpoch {
		t.Fatalf("unexpected secure welcome: %+v", welcome)
	}
	if _, _, ok := state.MediaSecurity(welcome.SessionID); !ok {
		t.Fatal("authenticated session has no media security context")
	}
}

func TestSecureMultiIdentityAdministrationOverControlConnections(t *testing.T) {
	identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "server-identity.json"))
	if err != nil {
		t.Fatal(err)
	}
	tlsConfig, err := identity.TLSConfig("Test")
	if err != nil {
		t.Fatal(err)
	}
	state := serverstate.New(config.Default())
	store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "administration.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	server := NewSecure("", "127.0.0.1:9001", state, slog.New(slog.NewTextHandler(io.Discard, nil)), identity, tlsConfig, false, store)
	_, masterPrivate, _ := ed25519.GenerateKey(cryptorand.Reader)
	_, adminPrivate, _ := ed25519.GenerateKey(cryptorand.Reader)
	_, userPrivate, _ := ed25519.GenerateKey(cryptorand.Reader)

	master := newSecureTestClient(t, server, tlsConfig, "Master", masterPrivate)
	defer master.conn.Close()
	master.send(t, protocol.TypeProvisionMaster, protocol.ProvisionMasterRequest{Token: token})
	master.receiveUntil(t, protocol.TypeCapabilities)

	admin := newSecureTestClient(t, server, tlsConfig, "Admin", adminPrivate)
	defer admin.conn.Close()
	master.send(t, protocol.TypeSetServerRole, protocol.ServerRoleRequest{ClientID: admin.clientID, Role: string(administration.RoleAdmin)})
	admin.receiveUntil(t, protocol.TypeCapabilities)
	admin.receiveUntil(t, protocol.TypeServerState)
	if stored, ok := store.User(admin.clientID); !ok || stored.ServerRole != administration.RoleAdmin {
		t.Fatalf("admin role was not persisted: %+v", stored)
	}

	admin.send(t, protocol.TypeCreateChannel, protocol.ChannelMutationRequest{Channel: protocol.Channel{Name: "Integration Root", SortOrder: 100}})
	rootChannel := channelFromSnapshot(t, admin.receiveUntil(t, protocol.TypeServerState), "Integration Root")
	admin.send(t, protocol.TypeCreateChannel, protocol.ChannelMutationRequest{Channel: protocol.Channel{ParentID: rootChannel.ID, Name: "Integration Child", SortOrder: 101}})
	childChannel := channelFromSnapshot(t, admin.receiveUntil(t, protocol.TypeServerState), "Integration Child")
	childChannel.Name = "Renamed Child"
	admin.send(t, protocol.TypeEditChannel, protocol.ChannelMutationRequest{Channel: childChannel})
	childChannel = channelFromSnapshot(t, admin.receiveUntil(t, protocol.TypeServerState), "Renamed Child")
	admin.send(t, protocol.TypeSetRoomNetworkPolicy, protocol.RoomNetworkPolicyRequest{ChannelID: rootChannel.ID, Policy: "force_sfu"})
	waitForRoomPolicy(t, state, rootChannel.ID, "force_sfu")

	user := newSecureTestClient(t, server, tlsConfig, "User", userPrivate)
	admin.send(t, protocol.TypeSetChannelAdmin, protocol.ChannelRoleRequest{ChannelID: childChannel.ID, ClientID: user.clientID, Enabled: true})
	var scopedCapabilities protocol.Capabilities
	if err := user.receiveUntil(t, protocol.TypeCapabilities).Decode(&scopedCapabilities); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(scopedCapabilities.NetworkPolicyChannelIDs, []uint64{childChannel.ID}) {
		t.Fatalf("out-of-room CHANNEL_ADMIN network-policy scope = %v, want [%d]", scopedCapabilities.NetworkPolicyChannelIDs, childChannel.ID)
	}
	if role := store.ChannelRole(childChannel.ID, user.clientID); role != administration.RoleChannelAdmin {
		t.Fatalf("channel admin role was not persisted: %q", role)
	}

	user.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: childChannel.ID})
	moved := user.receiveUserMovedUntil(t, user.sessionID, childChannel.ID)
	if moved.SessionID != user.sessionID || moved.ChannelID != childChannel.ID {
		t.Fatalf("user did not join child channel: %+v", moved)
	}
	user.send(t, protocol.TypeSetRoomNetworkPolicy, protocol.RoomNetworkPolicyRequest{ChannelID: childChannel.ID, Policy: "force_sfu"})
	user.receiveRouteUntil(t, childChannel.ID, "force_sfu")
	user.send(t, protocol.TypeSetRoomNetworkPolicy, protocol.RoomNetworkPolicyRequest{ChannelID: rootChannel.ID, Policy: "auto"})
	var policyError protocol.Error
	if err := user.receiveUntil(t, protocol.TypeError).Decode(&policyError); err != nil {
		t.Fatal(err)
	}
	if policyError.Code != "permission_denied" || state.EvaluateRoom(rootChannel.ID).RoomNetworkPolicy != "force_sfu" {
		t.Fatalf("channel admin escaped network-policy scope: error=%+v route=%+v", policyError, state.EvaluateRoom(rootChannel.ID))
	}
	admin.send(t, protocol.TypeAdminMute, protocol.TargetRequest{ClientID: user.clientID, SessionID: user.sessionID, Enabled: true})
	var changed protocol.UserStateChanged
	if err := user.receiveUntil(t, protocol.TypeUserStateChanged).Decode(&changed); err != nil {
		t.Fatal(err)
	}
	if changed.SessionID != user.sessionID || !changed.Muted || !changed.AdminMuted || state.CanSpeak(user.sessionID) {
		t.Fatalf("administrative mute did not take effect: %+v", changed)
	}
	admin.send(t, protocol.TypeMoveUser, protocol.TargetRequest{ClientID: user.clientID, SessionID: user.sessionID, ChannelID: rootChannel.ID})
	moved = user.receiveUserMovedUntil(t, user.sessionID, rootChannel.ID)
	if moved.ChannelID != rootChannel.ID {
		t.Fatalf("user was not moved to root channel: %+v", moved)
	}
	admin.send(t, protocol.TypeKickChannel, protocol.TargetRequest{ClientID: user.clientID, SessionID: user.sessionID})
	moved = user.receiveUserMovedUntil(t, user.sessionID, 0)
	if moved.ChannelID != 0 {
		t.Fatalf("channel kick did not remove user: %+v", moved)
	}
	admin.send(t, protocol.TypeDeleteChannel, protocol.ChannelRequest{ChannelID: childChannel.ID})
	admin.receiveUntil(t, protocol.TypeServerState)
	admin.send(t, protocol.TypeDeleteChannel, protocol.ChannelRequest{ChannelID: rootChannel.ID})
	admin.receiveUntil(t, protocol.TypeServerState)

	duplicate := newSecureTestClient(t, server, tlsConfig, "User second session", userPrivate)
	defer duplicate.conn.Close()
	if duplicate.clientID != user.clientID || duplicate.sessionID == user.sessionID {
		t.Fatalf("duplicate identity session mismatch: first=%+v second=%+v", user, duplicate)
	}
	admin.send(t, protocol.TypeBan, protocol.TargetRequest{ClientID: user.clientID, Reason: "integration test"})
	disconnected := make(map[uint64]bool)
	for range 2 {
		var left protocol.UserLeft
		if err := admin.receiveUntil(t, protocol.TypeUserLeft).Decode(&left); err != nil {
			t.Fatal(err)
		}
		disconnected[left.SessionID] = true
	}
	if !disconnected[user.sessionID] || !disconnected[duplicate.sessionID] {
		t.Fatalf("ban did not disconnect every session for identity: %+v", disconnected)
	}

	admin.send(t, protocol.TypeListBans, protocol.BanListRequest{Limit: 8})
	var bans protocol.BanList
	if err := admin.receiveUntil(t, protocol.TypeBanList).Decode(&bans); err != nil {
		t.Fatal(err)
	}
	if bans.Total != 1 || len(bans.Bans) != 1 || bans.Bans[0].ClientID != user.clientID {
		t.Fatalf("unexpected ban list: %+v", bans)
	}
	admin.send(t, protocol.TypeUnban, protocol.TargetRequest{ClientID: user.clientID})
	admin.send(t, protocol.TypeListBans, protocol.BanListRequest{Limit: 8})
	if err := admin.receiveUntil(t, protocol.TypeBanList).Decode(&bans); err != nil {
		t.Fatal(err)
	}
	if bans.Total != 0 || len(bans.Bans) != 0 {
		t.Fatalf("unban did not take effect: %+v", bans)
	}

	reconnected := newSecureTestClient(t, server, tlsConfig, "Renamed User", userPrivate)
	defer reconnected.conn.Close()
	if reconnected.clientID != user.clientID {
		t.Fatalf("renamed identity changed client ID: got %q want %q", reconnected.clientID, user.clientID)
	}
}

func TestMultiSessionModerationUsesExactSessionAndBanDisconnectsIdentity(t *testing.T) {
	state := serverstate.New(config.Default())
	store, _, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"admin", "user"} {
		if _, err := store.Admit(id, id+"-key", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.SetServerRole("admin", administration.RoleAdmin); err != nil {
		t.Fatal(err)
	}
	firstSender := &recordingSender{}
	secondSender := &recordingSender{}
	admin := state.AddAuthenticatedSession("Admin", "admin", string(administration.RoleAdmin), "", &recordingSender{}, nil, nil)
	first := state.AddAuthenticatedSession("User A", "user", string(administration.RoleUser), "", firstSender, nil, nil)
	second := state.AddAuthenticatedSession("User B", "user", string(administration.RoleUser), "", secondSender, nil, nil)
	if err := state.Join(first.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(second.ID, 2); err != nil {
		t.Fatal(err)
	}
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = store

	legacyMute, _ := protocol.NewEnvelope(protocol.TypeAdminMute, protocol.TargetRequest{ClientID: "user", Enabled: true})
	if err := server.dispatch(admin.ID, legacyMute, nil); err == nil {
		t.Fatal("ambiguous legacy target unexpectedly selected an arbitrary session")
	}
	mismatch, _ := protocol.NewEnvelope(protocol.TypeAdminMute, protocol.TargetRequest{ClientID: "admin", SessionID: first.ID, Enabled: true})
	if err := server.dispatch(admin.ID, mismatch, nil); err == nil {
		t.Fatal("mismatched session and client identity was accepted")
	}
	muteFirst, _ := protocol.NewEnvelope(protocol.TypeAdminMute, protocol.TargetRequest{ClientID: "user", SessionID: first.ID, Enabled: true})
	if err := server.dispatch(admin.ID, muteFirst, nil); err != nil {
		t.Fatalf("exact-session mute failed: %v", err)
	}
	if state.CanSpeak(first.ID) || !state.CanSpeak(second.ID) {
		t.Fatal("session-scoped mute affected the wrong identity session")
	}
	kickSecond, _ := protocol.NewEnvelope(protocol.TypeKickChannel, protocol.TargetRequest{ClientID: "user", SessionID: second.ID})
	if err := server.dispatch(admin.ID, kickSecond, nil); err != nil {
		t.Fatalf("exact-session channel kick failed: %v", err)
	}
	if state.ChannelOf(first.ID) != 1 || state.ChannelOf(second.ID) != 0 {
		t.Fatal("session-scoped channel kick affected the wrong identity session")
	}
	banIdentity, _ := protocol.NewEnvelope(protocol.TypeBan, protocol.TargetRequest{ClientID: "user", Reason: "multi-session test"})
	if err := server.dispatch(admin.ID, banIdentity, nil); err != nil {
		t.Fatalf("identity ban failed: %v", err)
	}
	if _, ok := state.SessionByID(first.ID); ok {
		t.Fatal("first banned identity session remained online")
	}
	if _, ok := state.SessionByID(second.ID); ok {
		t.Fatal("second banned identity session remained online")
	}
	for name, sender := range map[string]*recordingSender{"first": firstSender, "second": secondSender} {
		if code := lastErrorCode(t, sender.messages); code != "banned" {
			t.Fatalf("%s banned session received terminal error %q, want banned", name, code)
		}
	}
}

func TestAdministrationDispatchEnforcesRolesAndMutatesAuthoritativeState(t *testing.T) {
	cfg := config.Default()
	state := serverstate.New(cfg)
	channels := state.Snapshot().Channels
	store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"master", "admin", "user"} {
		if _, err := store.Admit(id, id+"-key", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	if err := store.SetServerRole("admin", administration.RoleAdmin); err != nil {
		t.Fatal(err)
	}
	master := state.AddAuthenticatedSession("Master", "master", string(administration.RoleMaster), "", &recordingSender{}, nil, nil)
	admin := state.AddAuthenticatedSession("Admin", "admin", string(administration.RoleAdmin), "", &recordingSender{}, nil, nil)
	userSender := &recordingSender{}
	user := state.AddAuthenticatedSession("User", "user", string(administration.RoleUser), "", userSender, nil, nil)
	for _, session := range []*serverstate.Session{master, admin, user} {
		if err := state.Join(session.ID, 2); err != nil {
			t.Fatal(err)
		}
	}
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = store
	rename, _ := protocol.NewEnvelope(protocol.TypeSetDisplayName, protocol.DisplayNameRequest{DisplayName: "Renamed User"})
	if err := server.dispatch(user.ID, rename, nil); err != nil {
		t.Fatalf("self display-name update failed: %v", err)
	}
	if current, _ := state.User(user.ID); current.Nickname != "Renamed User" {
		t.Fatalf("online display name was not updated: %+v", current)
	}
	if persisted, _ := store.User("user"); persisted.DisplayName != "Renamed User" {
		t.Fatalf("display name was not persisted: %+v", persisted)
	}
	if len(userSender.messages) == 0 || userSender.messages[len(userSender.messages)-1].Type != protocol.TypeServerState {
		t.Fatalf("display-name update did not broadcast an authoritative snapshot: %+v", userSender.messages)
	}
	policyCapability := false
	for _, capability := range server.capabilities("admin", 2) {
		policyCapability = policyCapability || capability == "network.policy.set"
	}
	if !policyCapability {
		t.Fatal("authorized ADMIN did not receive the room network policy capability")
	}
	if err := store.Ban("user", "abuse", "admin", nil); err != nil {
		t.Fatal(err)
	}
	banListSender := &recordingSender{}
	listBans, _ := protocol.NewEnvelope(protocol.TypeListBans, protocol.BanListRequest{Limit: 8})
	if err := server.dispatch(admin.ID, listBans, banListSender); err != nil {
		t.Fatalf("admin list bans failed: %v", err)
	}
	if len(banListSender.messages) != 1 || banListSender.messages[0].Type != protocol.TypeBanList {
		t.Fatalf("unexpected ban list response: %+v", banListSender.messages)
	}
	var listed protocol.BanList
	if err := banListSender.messages[0].Decode(&listed); err != nil {
		t.Fatal(err)
	}
	if listed.Total != 1 || len(listed.Bans) != 1 || listed.Bans[0].DisplayName != "Renamed User" {
		t.Fatalf("unexpected ban list payload: %+v", listed)
	}
	if err := server.dispatch(user.ID, listBans, nil); !errors.Is(err, administration.ErrForbidden) {
		t.Fatalf("user listed bans: %v", err)
	}
	mute, _ := protocol.NewEnvelope(protocol.TypeAdminMute, protocol.TargetRequest{ClientID: "user", Enabled: true})
	if err := server.dispatch(user.ID, mute, nil); !errors.Is(err, administration.ErrForbidden) {
		t.Fatalf("user moderation accepted: %v", err)
	}
	if err := server.dispatch(admin.ID, mute, nil); err != nil {
		t.Fatalf("admin mute failed: %v", err)
	}
	if state.CanSpeak(user.ID) {
		t.Fatal("administratively muted user can still speak")
	}
	for _, messageType := range []protocol.MessageType{protocol.TypeKickChannel, protocol.TypeKickServer, protocol.TypeBan} {
		request, _ := protocol.NewEnvelope(messageType, protocol.TargetRequest{ClientID: "admin", Reason: "self-target"})
		if err := server.dispatch(admin.ID, request, nil); !errors.Is(err, administration.ErrForbidden) {
			t.Fatalf("self-targeted %s accepted: %v", messageType, err)
		}
	}
	banMaster, _ := protocol.NewEnvelope(protocol.TypeBan, protocol.TargetRequest{ClientID: "master", Reason: "no"})
	if err := server.dispatch(admin.ID, banMaster, nil); !errors.Is(err, administration.ErrForbidden) {
		t.Fatalf("admin managed master: %v", err)
	}
	deleteChannel, _ := protocol.NewEnvelope(protocol.TypeDeleteChannel, protocol.ChannelRequest{ChannelID: 2})
	if err := server.dispatch(master.ID, deleteChannel, nil); err != nil {
		t.Fatalf("master delete channel: %v", err)
	}
	if state.ChannelOf(user.ID) == 2 || state.ChannelOf(user.ID) == 0 {
		t.Fatal("channel deletion did not move user to safe fallback")
	}
	if state.RoomSize(2) != 0 {
		t.Fatal("deleted channel retained members")
	}
}

func TestNetworkPolicyCapabilitiesAreScopedByTargetChannel(t *testing.T) {
	cfg := config.Default()
	state := serverstate.New(cfg)
	channels := state.Snapshot().Channels
	store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"master", "admin", "channel-admin", "user"} {
		if _, err := store.Admit(id, id+"-key", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	if err := store.SetServerRole("admin", administration.RoleAdmin); err != nil {
		t.Fatal(err)
	}
	if err := store.SetChannelRole(channels[1].ID, "channel-admin", administration.RoleChannelAdmin); err != nil {
		t.Fatal(err)
	}
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = store

	allChannelIDs := make([]uint64, 0, len(channels))
	for _, channel := range channels {
		allChannelIDs = append(allChannelIDs, channel.ID)
	}
	if got := server.networkPolicyChannelIDs("master"); !slices.Equal(got, allChannelIDs) {
		t.Fatalf("MASTER network-policy scope = %v, want %v", got, allChannelIDs)
	}
	if got := server.networkPolicyChannelIDs("admin"); !slices.Equal(got, allChannelIDs) {
		t.Fatalf("ADMIN network-policy scope = %v, want %v", got, allChannelIDs)
	}
	if got := server.networkPolicyChannelIDs("channel-admin"); !slices.Equal(got, []uint64{channels[1].ID}) {
		t.Fatalf("CHANNEL_ADMIN network-policy scope = %v, want [%d]", got, channels[1].ID)
	}
	if got := server.networkPolicyChannelIDs("user"); len(got) != 0 {
		t.Fatalf("USER unexpectedly received network-policy scope %v", got)
	}
	channelAdmin := state.AddAuthenticatedSession("Channel Admin", "channel-admin", string(administration.RoleUser), "", &recordingSender{}, nil, nil)
	member := state.AddAuthenticatedSession("Member", "user", string(administration.RoleUser), "", &recordingSender{}, nil, nil)
	if err := state.Join(channelAdmin.ID, channels[1].ID); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(member.ID, channels[1].ID); err != nil {
		t.Fatal(err)
	}
	mute, _ := protocol.NewEnvelope(protocol.TypeAdminMute, protocol.TargetRequest{ClientID: "user", SessionID: member.ID, Enabled: true})
	if err := server.dispatch(channelAdmin.ID, mute, nil); err != nil {
		t.Fatalf("channel-admin mute failed: %v", err)
	}
	if state.CanSpeak(member.ID) {
		t.Fatal("channel mute did not suppress the member in its channel")
	}
	if err := state.Move(member.ID, channels[0].ID); err != nil {
		t.Fatal(err)
	}
	if !state.CanSpeak(member.ID) {
		t.Fatal("channel-admin mute escaped its authorized channel")
	}
}

type recordingSender struct{ messages []protocol.Envelope }

func (s *recordingSender) Send(message protocol.Envelope) error {
	s.messages = append(s.messages, message)
	return nil
}

func TestLargeSFURoomSkipsQuadraticCandidateFanoutAndResumesAfterShrink(t *testing.T) {
	state := serverstate.New(config.Default())
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	type participant struct {
		id     uint64
		sender *recordingSender
	}
	participants := make([]participant, 0, 12)
	for index := 0; index < 12; index++ {
		sender := &recordingSender{}
		session := state.AddSession("member", sender)
		if err := state.Join(session.ID, 1); err != nil {
			t.Fatal(err)
		}
		if err := state.SetCandidates(session.ID, []protocol.Candidate{{Kind: "host", Address: fmt.Sprintf("192.0.2.%d:4000", index+1)}}); err != nil {
			t.Fatal(err)
		}
		participants = append(participants, participant{id: session.ID, sender: sender})
	}
	for _, member := range participants {
		server.sendPeerCandidates(member.id)
		server.broadcastCandidate(member.id)
	}
	for index, member := range participants {
		if got := len(member.sender.messages); got != 0 {
			t.Fatalf("large-room member %d received %d unnecessary candidate messages", index, got)
		}
	}
	for _, member := range participants[6:] {
		if err := state.Leave(member.id); err != nil {
			t.Fatal(err)
		}
	}
	server.broadcastRoomCandidates(1)
	for index, member := range participants[:6] {
		if got := len(member.sender.messages); got != 5 {
			t.Fatalf("shrunk-room member %d received %d candidates, want 5", index, got)
		}
	}
}

func TestAvatarUpdateBroadcastsTheNewReferenceToEveryClient(t *testing.T) {
	state := serverstate.New(config.Default())
	store, _, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, clientID := range []string{"actor", "observer"} {
		if _, err := store.Admit(clientID, clientID+"-key", clientID, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	actorSender, observerSender := &recordingSender{}, &recordingSender{}
	actor := state.AddAuthenticatedSession("Actor", "actor", string(administration.RoleUser), "", actorSender, nil, nil)
	state.AddAuthenticatedSession("Observer", "observer", string(administration.RoleUser), "", observerSender, nil, nil)
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = store

	request, _ := protocol.NewEnvelope(protocol.TypeSetAvatar, protocol.AvatarRequest{Avatar: "K"})
	if err := server.dispatch(actor.ID, request, actorSender); err != nil {
		t.Fatal(err)
	}
	for name, sender := range map[string]*recordingSender{"actor": actorSender, "observer": observerSender} {
		message := mustEnvelope(t, protocol.TypeServerState, sender.messages)
		var snapshot protocol.ServerState
		if err := message.Decode(&snapshot); err != nil {
			t.Fatal(err)
		}
		found := false
		for _, user := range snapshot.Users {
			if user.SessionID == actor.ID {
				found = true
				if user.Avatar != "K" {
					t.Fatalf("%s received avatar %q, want K", name, user.Avatar)
				}
			}
		}
		if !found {
			t.Fatalf("%s did not receive the updated user", name)
		}
	}
}

func TestTemporaryChannelCreationAutoJoinsAndLeavingDeletesIt(t *testing.T) {
	state := serverstate.New(config.Default())
	store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Admit("master", "master-key", "Master", time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	sender := &recordingSender{}
	master := state.AddAuthenticatedSession("Master", "master", string(administration.RoleMaster), "", sender, nil, nil)
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.administration = store

	create, _ := protocol.NewEnvelope(protocol.TypeCreateChannel, protocol.ChannelMutationRequest{Channel: protocol.Channel{
		Name: "Quick room", SortOrder: 100, Temporary: true, AudioQualityPolicy: "quality_10",
	}})
	if err := server.dispatch(master.ID, create, sender); err != nil {
		t.Fatal(err)
	}
	channelID := state.ChannelOf(master.ID)
	if channelID == 0 {
		t.Fatal("temporary channel creator was not automatically joined")
	}
	created := channelFromSnapshot(t, mustEnvelope(t, protocol.TypeServerState, sender.messages), "Quick room")
	if created.ID != channelID || !created.Temporary || created.AudioQualityPolicy != "quality_10" {
		t.Fatalf("unexpected temporary channel: %+v", created)
	}

	leave, _ := protocol.NewEnvelope(protocol.TypeLeaveChannel, struct{}{})
	if err := server.dispatch(master.ID, leave, sender); err != nil {
		t.Fatal(err)
	}
	for _, channel := range state.Snapshot().Channels {
		if channel.ID == channelID {
			t.Fatalf("empty temporary channel remained in runtime state: %+v", channel)
		}
	}
	for _, channel := range store.Channels() {
		if channel.ID == channelID {
			t.Fatalf("empty temporary channel remained in administration state: %+v", channel)
		}
	}
}

func mustEnvelope(t *testing.T, kind protocol.MessageType, messages []protocol.Envelope) protocol.Envelope {
	t.Helper()
	for index := len(messages) - 1; index >= 0; index-- {
		if messages[index].Type == kind {
			return messages[index]
		}
	}
	t.Fatalf("missing %s envelope", kind)
	return protocol.Envelope{}
}

func TestTextChatBroadcastsOnlyInsideChannelAndRejectsSpoofableMetadata(t *testing.T) {
	state := serverstate.New(config.Default())
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	aliceSender, bobSender, outsiderSender := &recordingSender{}, &recordingSender{}, &recordingSender{}
	alice := state.AddAuthenticatedSession("Alice", "client-a", "USER", "", aliceSender, nil, nil)
	bob := state.AddSession("Bob", bobSender)
	outsider := state.AddSession("Outside", outsiderSender)
	if err := state.Join(alice.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(bob.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(outsider.ID, 2); err != nil {
		t.Fatal(err)
	}
	request, _ := protocol.NewEnvelope(protocol.TypeSendTextMessage, map[string]any{
		"text": " hello ", "sender_name": "Mallory", "channel_id": 2,
	})
	if err := server.dispatch(alice.ID, request, nil); err != nil {
		t.Fatal(err)
	}
	for name, sender := range map[string]*recordingSender{"alice": aliceSender, "bob": bobSender} {
		if len(sender.messages) != 1 || sender.messages[0].Type != protocol.TypeTextMessage {
			t.Fatalf("%s did not receive channel message: %+v", name, sender.messages)
		}
		var message protocol.TextMessage
		if err := sender.messages[0].Decode(&message); err != nil {
			t.Fatal(err)
		}
		if message.SenderName != "Alice" || message.ChannelID != 1 || message.Text != "hello" {
			t.Fatalf("client spoofed authoritative metadata: %+v", message)
		}
	}
	if len(outsiderSender.messages) != 0 {
		t.Fatalf("message leaked outside channel: %+v", outsiderSender.messages)
	}
}

func lastErrorCode(t *testing.T, messages []protocol.Envelope) string {
	t.Helper()
	for index := len(messages) - 1; index >= 0; index-- {
		if messages[index].Type != protocol.TypeError {
			continue
		}
		var serverError protocol.Error
		if err := messages[index].Decode(&serverError); err != nil {
			t.Fatal(err)
		}
		return serverError.Code
	}
	return ""
}

func TestDisconnectWithErrorSendsReasonBeforeRemovingSession(t *testing.T) {
	state := serverstate.New(config.Default())
	sender := &recordingSender{}
	session := state.AddAuthenticatedSession("User", "user", string(administration.RoleUser), "", sender, nil, nil)
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))

	server.disconnectWithError(session.ID, sender, "kicked_from_server", "kicked by administrator")

	if _, ok := state.SessionByID(session.ID); ok {
		t.Fatal("disconnected session remained online")
	}
	if len(sender.messages) == 0 || sender.messages[0].Type != protocol.TypeError {
		t.Fatalf("terminal error was not sent before disconnect: %+v", sender.messages)
	}
	var terminal protocol.Error
	if err := sender.messages[0].Decode(&terminal); err != nil {
		t.Fatal(err)
	}
	if terminal.Code != "kicked_from_server" || terminal.Message != "kicked by administrator" {
		t.Fatalf("unexpected terminal error: %+v", terminal)
	}
}

func newTestClient(t *testing.T, server *Server, nickname string) *testClient {
	t.Helper()
	serverConn, clientConn := net.Pipe()
	go server.handle(serverConn)
	client := &testClient{conn: clientConn, enc: json.NewEncoder(clientConn), dec: json.NewDecoder(clientConn)}
	clientConn.SetDeadline(time.Now().Add(2 * time.Second))
	hello, _ := protocol.NewEnvelope(protocol.TypeHello, protocol.Hello{Nickname: nickname})
	if err := client.enc.Encode(hello); err != nil {
		t.Fatal(err)
	}
	if message := client.receive(t); message.Type != protocol.TypeWelcome {
		t.Fatalf("expected welcome, got %s", message.Type)
	}
	if message := client.receive(t); message.Type != protocol.TypeServerState {
		t.Fatalf("expected server state, got %s", message.Type)
	}
	return client
}

func newSecureTestClient(t *testing.T, server *Server, tlsConfig *tls.Config, nickname string, privateKey ed25519.PrivateKey) *testClient {
	t.Helper()
	serverConn, clientConn := net.Pipe()
	go server.handle(tls.Server(serverConn, tlsConfig))
	clientTLS := tls.Client(clientConn, &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}) // EdenVoice identity is authenticated by the signed challenge.
	clientTLS.SetDeadline(time.Now().Add(3 * time.Second))
	client := &testClient{conn: clientTLS, enc: json.NewEncoder(clientTLS), dec: json.NewDecoder(clientTLS)}
	publicKey := privateKey.Public().(ed25519.PublicKey)
	client.clientID = security.ClientID(publicKey)
	clientEphemeral, err := ecdh.X25519().GenerateKey(cryptorand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	hello, _ := protocol.NewEnvelope(protocol.TypeHello, protocol.Hello{
		Nickname: nickname, ProtocolVersion: security.ProtocolVersion,
		SecurityCapabilities: requiredSecurityCapabilities(), ClientID: client.clientID,
		ClientPublicKey: base64.RawStdEncoding.EncodeToString(publicKey), ClientEphemeralKey: base64.RawStdEncoding.EncodeToString(clientEphemeral.PublicKey().Bytes()),
	})
	if err := client.enc.Encode(hello); err != nil {
		t.Fatal(err)
	}
	challengeEnvelope := client.receive(t)
	if challengeEnvelope.Type != protocol.TypeIdentityChallenge {
		t.Fatalf("expected identity challenge, got %s", challengeEnvelope.Type)
	}
	var challenge protocol.IdentityChallenge
	if err := challengeEnvelope.Decode(&challenge); err != nil {
		t.Fatal(err)
	}
	challengeBytes, err := base64.RawStdEncoding.DecodeString(challenge.Challenge)
	if err != nil {
		t.Fatal(err)
	}
	serverPublic, err := base64.RawStdEncoding.DecodeString(challenge.ServerPublicKey)
	if err != nil {
		t.Fatal(err)
	}
	serverEphemeral, err := base64.RawStdEncoding.DecodeString(challenge.ServerEphemeralKey)
	if err != nil {
		t.Fatal(err)
	}
	transcript := security.AuthTranscript(challengeBytes, serverPublic, clientEphemeral.PublicKey().Bytes(), serverEphemeral)
	client.send(t, protocol.TypeIdentityResponse, protocol.IdentityResponse{Signature: base64.RawStdEncoding.EncodeToString(ed25519.Sign(privateKey, transcript))})
	var welcome protocol.Welcome
	if err := client.receiveUntil(t, protocol.TypeWelcome).Decode(&welcome); err != nil {
		t.Fatal(err)
	}
	client.sessionID = welcome.SessionID
	if welcome.ClientID != client.clientID {
		t.Fatalf("authenticated client ID mismatch: got %q want %q", welcome.ClientID, client.clientID)
	}
	client.receiveUntil(t, protocol.TypeServerState)
	return client
}

func (c *testClient) receive(t *testing.T) protocol.Envelope {
	t.Helper()
	if err := c.conn.SetReadDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatal(err)
	}
	var message protocol.Envelope
	if err := c.dec.Decode(&message); err != nil {
		t.Fatal(err)
	}
	return message
}

func (c *testClient) receiveUntil(t *testing.T, kind protocol.MessageType) protocol.Envelope {
	t.Helper()
	for range 64 {
		message := c.receive(t)
		if message.Type == kind {
			return message
		}
	}
	t.Fatalf("did not receive %s", kind)
	return protocol.Envelope{}
}

func (c *testClient) receiveUserMovedUntil(t *testing.T, sessionID, channelID uint64) protocol.UserMoved {
	t.Helper()
	for range 64 {
		message := c.receiveUntil(t, protocol.TypeUserMoved)
		var moved protocol.UserMoved
		if err := message.Decode(&moved); err != nil {
			t.Fatal(err)
		}
		if moved.SessionID == sessionID && moved.ChannelID == channelID {
			return moved
		}
	}
	t.Fatalf("did not receive user_moved for session %d and channel %d", sessionID, channelID)
	return protocol.UserMoved{}
}

func (c *testClient) receiveRouteUntil(t *testing.T, channelID uint64, policy string) protocol.RouteUpdate {
	t.Helper()
	for range 64 {
		message := c.receiveUntil(t, protocol.TypeRouteUpdate)
		var route protocol.RouteUpdate
		if err := message.Decode(&route); err != nil {
			t.Fatal(err)
		}
		if route.RoomID == channelID && route.RoomNetworkPolicy == policy {
			return route
		}
	}
	t.Fatalf("did not receive route update for channel %d and policy %q", channelID, policy)
	return protocol.RouteUpdate{}
}

func waitForRoomPolicy(t *testing.T, state *serverstate.State, channelID uint64, policy string) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if state.EvaluateRoom(channelID).RoomNetworkPolicy == policy {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatalf("room %d did not adopt policy %q", channelID, policy)
}

func channelFromSnapshot(t *testing.T, message protocol.Envelope, name string) protocol.Channel {
	t.Helper()
	var snapshot protocol.ServerState
	if err := message.Decode(&snapshot); err != nil {
		t.Fatal(err)
	}
	for _, channel := range snapshot.Channels {
		if channel.Name == name {
			return channel
		}
	}
	t.Fatalf("channel %q not found in snapshot: %+v", name, snapshot.Channels)
	return protocol.Channel{}
}

func (c *testClient) send(t *testing.T, kind protocol.MessageType, value any) {
	t.Helper()
	if err := c.conn.SetWriteDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatal(err)
	}
	message, err := protocol.NewEnvelope(kind, value)
	if err != nil {
		t.Fatal(err)
	}
	if err := c.enc.Encode(message); err != nil {
		t.Fatal(err)
	}
}

func TestConnSenderBoundsSlowConsumerBackpressure(t *testing.T) {
	serverConn, clientConn := net.Pipe()
	defer clientConn.Close()
	sender := newConnSender(serverConn)
	defer sender.Close()
	message, _ := protocol.NewEnvelope(protocol.TypePong, protocol.Ping{Nonce: 1})
	var queueError error
	for range controlSendQueueSize + 2 {
		if err := sender.Send(message); err != nil {
			queueError = err
			break
		}
	}
	if queueError == nil {
		t.Fatal("slow consumer did not hit the bounded send queue")
	}
}

func TestBroadcastRouteSuppressesUnchangedUpdates(t *testing.T) {
	state := serverstate.New(config.Default())
	sender := &recordingSender{}
	session := state.AddSession("A", sender)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	server := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server.broadcastRoute(1)
	server.broadcastRoute(1)
	if len(sender.messages) != 1 {
		t.Fatalf("unchanged route was broadcast %d times", len(sender.messages))
	}
}

func TestControlHandshakeJoinAndDisconnect(t *testing.T) {
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	server := New("", "127.0.0.1:9001", serverstate.New(config.Default()), logger)
	alice := newTestClient(t, server, "Alice")
	defer alice.conn.Close()
	bob := newTestClient(t, server, "Bob")
	joined := alice.receive(t)
	if joined.Type != protocol.TypeUserJoined {
		t.Fatalf("expected user_joined, got %s", joined.Type)
	}
	bob.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 2})
	for name, client := range map[string]*testClient{"alice": alice, "bob": bob} {
		moved := client.receive(t)
		if moved.Type != protocol.TypeUserMoved {
			t.Fatalf("%s expected user_moved, got %s", name, moved.Type)
		}
		var event protocol.UserMoved
		if err := moved.Decode(&event); err != nil {
			t.Fatal(err)
		}
		if event.ChannelID != 2 {
			t.Fatalf("unexpected channel %d", event.ChannelID)
		}
	}
	if err := bob.conn.Close(); err != nil {
		t.Fatal(err)
	}
	left := alice.receive(t)
	if left.Type != protocol.TypeUserLeft {
		t.Fatalf("expected user_left, got %s", left.Type)
	}
}

func TestControlRejectsOversizedHello(t *testing.T) {
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	server := New("", "", serverstate.New(config.Default()), logger)
	serverConn, clientConn := net.Pipe()
	defer clientConn.Close()
	go server.handle(serverConn)
	clientConn.SetDeadline(time.Now().Add(2 * time.Second))
	oversized := make([]byte, protocol.MaxControlMessageBytes+1)
	for i := range oversized {
		oversized[i] = 'a'
	}
	oversized[len(oversized)-1] = '\n'
	writeDone := make(chan error, 1)
	go func() { _, err := clientConn.Write(oversized); writeDone <- err }()
	var response protocol.Envelope
	if err := json.NewDecoder(clientConn).Decode(&response); err != nil {
		t.Fatal(err)
	}
	if response.Type != protocol.TypeError {
		t.Fatalf("expected error, got %s", response.Type)
	}
	<-writeDone
}

func TestHeartbeatCleanupClosesControlConnection(t *testing.T) {
	state := serverstate.New(config.Default())
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	server := New("", "", state, logger)
	serverConn, clientConn := net.Pipe()
	defer clientConn.Close()
	sender := newConnSender(serverConn)
	defer sender.Close()
	session := state.AddSession("Alice", sender)
	server.disconnect(session.ID, true)
	clientConn.SetReadDeadline(time.Now().Add(time.Second))
	buffer := make([]byte, 1)
	if _, err := clientConn.Read(buffer); err == nil {
		t.Fatal("expired control connection remained open")
	}
	if _, ok := state.User(session.ID); ok {
		t.Fatal("expired session remained in state")
	}
}

func TestAudioQoEFeedbackIsForwardedOnlyToTheRoomSender(t *testing.T) {
	state := serverstate.New(config.Default())
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	server := New("", "", state, logger)
	receiverSender := &recordingSender{}
	senderSender := &recordingSender{}
	receiver := state.AddSession("receiver", receiverSender)
	sender := state.AddSession("sender", senderSender)
	if err := state.Join(receiver.ID, 1); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(sender.ID, 1); err != nil {
		t.Fatal(err)
	}
	observed := uint64(25)
	feedback := protocol.AudioQoEFeedback{ObservedPackets: &observed, PeerID: sender.ID, WindowMS: 500, EffectiveAudioLossPercent: 7.5, MaxConsecutiveLoss: 3}
	message, _ := protocol.NewEnvelope(protocol.TypeAudioQoEFeedback, feedback)
	if err := server.dispatch(receiver.ID, message, nil); err != nil {
		t.Fatal(err)
	}
	if len(senderSender.messages) != 1 {
		t.Fatalf("sender received %d feedback messages", len(senderSender.messages))
	}
	var forwarded protocol.AudioQoEFeedback
	if err := senderSender.messages[0].Decode(&forwarded); err != nil {
		t.Fatal(err)
	}
	if forwarded.PeerID != receiver.ID || forwarded.EffectiveAudioLossPercent != 7.5 || forwarded.ObservedPackets == nil || *forwarded.ObservedPackets != observed {
		t.Fatalf("unexpected forwarded feedback: %+v", forwarded)
	}

	outsiderSender := &recordingSender{}
	outsider := state.AddSession("outsider", outsiderSender)
	if err := state.Join(outsider.ID, 2); err != nil {
		t.Fatal(err)
	}
	feedback.PeerID = outsider.ID
	message, _ = protocol.NewEnvelope(protocol.TypeAudioQoEFeedback, feedback)
	if err := server.dispatch(receiver.ID, message, nil); err != nil {
		t.Fatalf("best-effort cross-room feedback should be dropped silently: %v", err)
	}
	if len(outsiderSender.messages) != 0 {
		t.Fatalf("cross-room receiver got %d feedback messages", len(outsiderSender.messages))
	}
}
