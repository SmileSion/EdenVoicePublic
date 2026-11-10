package administration

import (
	"bytes"
	"encoding/base64"
	"errors"
	"image"
	"image/color"
	"image/jpeg"
	"os"
	"path/filepath"
	"testing"
	"time"

	"edenvoice/server/internal/protocol"
)

func TestDeletedAndTemporaryRoomIDsAreNeverReusedAfterRestart(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, err := LoadOrCreate(path, []protocol.Channel{{ID: 10, Name: "Lobby"}})
	if err != nil {
		t.Fatal(err)
	}
	deleted, err := store.CreateChannel(protocol.Channel{Name: "old room"})
	if err != nil {
		t.Fatal(err)
	}
	if err := store.DeleteChannel(deleted.ID); err != nil {
		t.Fatal(err)
	}
	temporary, err := store.CreateChannel(protocol.Channel{Name: "temporary", Temporary: true})
	if err != nil {
		t.Fatal(err)
	}
	if temporary.ID <= deleted.ID {
		t.Fatal("deleted room ID reused")
	}
	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	created, err := reloaded.CreateChannel(protocol.Channel{Name: "new room"})
	if err != nil {
		t.Fatal(err)
	}
	if created.ID <= temporary.ID {
		t.Fatal("temporary room ID reused after restart")
	}
	if _, err := reloaded.CreateChannel(protocol.Channel{ID: deleted.ID, Name: "explicit reuse"}); err == nil {
		t.Fatal("explicit reuse of old history accepted")
	}
}

func TestLegacyRoomIDHighWaterIsRecovered(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	if err := os.WriteFile(path, []byte(`{"version":1,"channels":{"21":{"id":21,"name":"existing"}}}`), 0600); err != nil {
		t.Fatal(err)
	}
	store, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	room, err := store.CreateChannel(protocol.Channel{Name: "new"})
	if err != nil || room.ID != 22 {
		t.Fatalf("legacy IDs not migrated: %+v %v", room, err)
	}
}

func TestCapabilityWireNamesStayCompatibleWithClient(t *testing.T) {
	if got := string(ActionNetworkPolicySet); got != "network.policy.set" {
		t.Fatalf("network policy capability wire name drifted: %q", got)
	}
}

func TestCustomAvatarPersistsByReferenceAndCanBeFetched(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, _ := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	now := time.Now().Round(0)
	if _, err := store.Admit("client", "public-key", "Alice", now); err != nil {
		t.Fatal(err)
	}
	avatar := image.NewRGBA(image.Rect(0, 0, protocol.AvatarSide, protocol.AvatarSide))
	for y := 0; y < protocol.AvatarSide; y++ {
		for x := 0; x < protocol.AvatarSide; x++ {
			avatar.SetRGBA(x, y, color.RGBA{R: uint8(x), G: uint8(y), B: 120, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := jpeg.Encode(&encoded, avatar, &jpeg.Options{Quality: 70}); err != nil {
		t.Fatal(err)
	}
	upload := protocol.CustomAvatarPrefix + base64.StdEncoding.EncodeToString(encoded.Bytes())
	if err := store.UpdateAvatar("client", upload, now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	user, _ := store.User("client")
	if user.Avatar == upload || user.AvatarData != upload {
		t.Fatalf("custom avatar was not split into reference and data: %+v", user)
	}
	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	data, ok := reloaded.AvatarData(user.Avatar)
	if !ok || data != upload {
		t.Fatal("custom avatar data was not available after restart")
	}
}

func TestRolesBansAndChannelRolesPersist(t *testing.T) {
	path := filepath.Join(t.TempDir(), "administration.json")
	store, token, err := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}, {ID: 2, Name: "Games"}})
	if err != nil || token == "" {
		t.Fatalf("create: token=%q err=%v", token, err)
	}
	for _, id := range []string{"master", "admin", "channel-admin", "user"} {
		if _, err := store.Admit(id, id+"-key", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	if err := store.SetServerRole("admin", RoleAdmin); err != nil {
		t.Fatal(err)
	}
	if err := store.SetChannelRole(2, "channel-admin", RoleChannelAdmin); err != nil {
		t.Fatal(err)
	}
	if err := store.Ban("user", "abuse", "admin", nil); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(path, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("administration permissions were not repaired: info=%v err=%v", info, err)
	}
	directoryInfo, err := os.Stat(filepath.Dir(path))
	if err != nil || directoryInfo.Mode().Perm() != 0o700 {
		t.Fatalf("administration directory permissions were not repaired: info=%v err=%v", directoryInfo, err)
	}
	if user, _ := reloaded.User("master"); user.ServerRole != RoleMaster {
		t.Fatal("master role not persisted")
	}
	if err := reloaded.Authorize("channel-admin", ActionMemberMute, Resource{ChannelID: 2, TargetClientID: "user"}); err != nil {
		t.Fatal(err)
	}
	if _, err := reloaded.Admit("user", "user-key", "renamed", time.Now()); !errors.Is(err, ErrBanned) {
		t.Fatalf("ban not enforced: %v", err)
	}
	bans := reloaded.Bans(time.Now())
	if len(bans) != 1 || bans[0].ClientID != "user" || bans[0].Reason != "abuse" {
		t.Fatalf("unexpected persisted ban list: %+v", bans)
	}
	if err := reloaded.Unban("user"); err != nil {
		t.Fatal(err)
	}
	if _, err := reloaded.Admit("user", "user-key", "renamed", time.Now()); err != nil {
		t.Fatalf("unban not enforced: %v", err)
	}
}

func TestIdentityMetadataAndLocalMasterRecoveryPersist(t *testing.T) {
	path := filepath.Join(t.TempDir(), "administration.json")
	store, token, err := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	if err != nil {
		t.Fatal(err)
	}
	firstSeen := time.Now().Add(-time.Hour).Round(0)
	if _, err := store.Admit("old-master", "old-key", "Old Master", firstSeen); err != nil {
		t.Fatal(err)
	}
	if err := store.ProvisionMaster("old-master", token); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Admit("replacement", "replacement-key", "Replacement", firstSeen); err != nil {
		t.Fatal(err)
	}
	lastSeen := firstSeen.Add(30 * time.Minute)
	if _, err := store.Admit("replacement", "replacement-key", "Renamed Replacement", lastSeen); err != nil {
		t.Fatal(err)
	}
	if err := store.RecoverMaster("replacement"); err != nil {
		t.Fatal(err)
	}

	reloaded, generatedToken, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	if generatedToken != "" {
		t.Fatal("restart generated a provision token despite a recovered MASTER")
	}
	oldMaster, _ := reloaded.User("old-master")
	if oldMaster.ServerRole != RoleUser {
		t.Fatalf("recovery did not demote the previous MASTER: %+v", oldMaster)
	}
	replacement, ok := reloaded.User("replacement")
	if !ok || replacement.ServerRole != RoleMaster || replacement.PublicKey != "replacement-key" || replacement.DisplayName != "Renamed Replacement" {
		t.Fatalf("recovered identity metadata did not persist: %+v", replacement)
	}
	if !replacement.FirstSeen.Equal(firstSeen) || !replacement.LastSeen.Equal(lastSeen) {
		t.Fatalf("identity timestamps did not persist: %+v", replacement)
	}
}

func TestAuthorizationProtectsMasterAndChannelScope(t *testing.T) {
	store, token, _ := LoadOrCreate(filepath.Join(t.TempDir(), "state.json"), []protocol.Channel{{ID: 1, Name: "A"}, {ID: 2, Name: "B"}})
	for _, id := range []string{"master", "admin", "channel-admin", "user"} {
		_, _ = store.Admit(id, id, id, time.Now())
	}
	_ = store.ProvisionMaster("master", token)
	_ = store.SetServerRole("admin", RoleAdmin)
	_ = store.SetChannelRole(1, "channel-admin", RoleChannelAdmin)
	if err := store.Authorize("admin", ActionMemberBan, Resource{TargetClientID: "master"}); !errors.Is(err, ErrForbidden) {
		t.Fatalf("admin managed master: %v", err)
	}
	if err := store.Authorize("channel-admin", ActionMemberMute, Resource{ChannelID: 2, TargetClientID: "user"}); !errors.Is(err, ErrForbidden) {
		t.Fatalf("channel admin escaped scope: %v", err)
	}
	if err := store.Authorize("user", ActionChannelCreate, Resource{}); !errors.Is(err, ErrForbidden) {
		t.Fatalf("user created channel: %v", err)
	}
	if err := store.Authorize("admin", ActionMemberBanList, Resource{}); err != nil {
		t.Fatalf("admin could not list bans: %v", err)
	}
	if err := store.Authorize("channel-admin", ActionMemberBanList, Resource{ChannelID: 1}); !errors.Is(err, ErrForbidden) {
		t.Fatalf("channel admin listed server bans: %v", err)
	}
	if scope, err := store.AuthorizeMute("master", Resource{ChannelID: 1, TargetClientID: "user"}); err != nil || scope != MuteScopeServer {
		t.Fatalf("master mute did not resolve to server scope: scope=%v err=%v", scope, err)
	}
	if scope, err := store.AuthorizeMute("channel-admin", Resource{ChannelID: 1, TargetClientID: "user"}); err != nil || scope != MuteScopeChannel {
		t.Fatalf("channel admin mute did not resolve to channel scope: scope=%v err=%v", scope, err)
	}
}

func TestDisplayNameUpdatePersistsWithoutChangingIdentity(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, _ := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	firstSeen := time.Now().Add(-time.Hour).Round(0)
	if _, err := store.Admit("client", "public-key", "old", firstSeen); err != nil {
		t.Fatal(err)
	}
	if err := store.UpdateDisplayName("client", "new", firstSeen.Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	user, ok := reloaded.User("client")
	if !ok || user.DisplayName != "new" || user.PublicKey != "public-key" || !user.FirstSeen.Equal(firstSeen) {
		t.Fatalf("unexpected persisted identity after rename: %+v", user)
	}
}

func TestAvatarUpdatePersistsWithIdentity(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, _ := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	now := time.Now().Round(0)
	if _, err := store.Admit("client", "public-key", "Alice", now); err != nil {
		t.Fatal(err)
	}
	if err := store.UpdateAvatar("client", "K", now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if err := store.UpdateAvatar("client", "unsupported", now.Add(2*time.Minute)); err == nil {
		t.Fatal("unsupported avatar was persisted")
	}
	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	user, ok := reloaded.User("client")
	if !ok || user.Avatar != "K" || user.PublicKey != "public-key" {
		t.Fatalf("unexpected persisted avatar identity: %+v", user)
	}
}

func TestDeletingChannelRemovesRolesAcrossRestart(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, _ := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}, {ID: 2, Name: "Games"}})
	_, _ = store.Admit("operator", "key", "Operator", time.Now())
	_ = store.SetChannelRole(2, "operator", RoleChannelAdmin)
	if err := store.DeleteChannel(2); err != nil {
		t.Fatal(err)
	}
	reloaded, _, _ := LoadOrCreate(path, nil)
	if err := reloaded.Authorize("operator", ActionChannelEdit, Resource{ChannelID: 2}); !errors.Is(err, ErrForbidden) {
		t.Fatalf("stale role remained: %v", err)
	}
}

func TestChannelNetworkPolicyPersistsAndSurvivesOrdinaryEdits(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, err := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	if err != nil {
		t.Fatal(err)
	}
	if err := store.SetChannelNetworkPolicy(1, "force_sfu"); err != nil {
		t.Fatal(err)
	}
	if err := store.EditChannel(protocol.Channel{ID: 1, Name: "Renamed", NetworkPolicy: "force_p2p"}); err != nil {
		t.Fatal(err)
	}

	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	channels := reloaded.Channels()
	if len(channels) != 1 || channels[0].Name != "Renamed" || channels[0].NetworkPolicy != "force_sfu" {
		t.Fatalf("network policy did not survive edit and restart: %+v", channels)
	}
	if err := reloaded.SetChannelNetworkPolicy(1, "invalid"); err == nil {
		t.Fatal("accepted an invalid network policy")
	}
}

func TestEditChannelRejectsUnknownParentAndHierarchyCycle(t *testing.T) {
	store, _, err := LoadOrCreate(filepath.Join(t.TempDir(), "state.json"), []protocol.Channel{
		{ID: 1, Name: "Root"},
		{ID: 2, ParentID: 1, Name: "Child"},
		{ID: 3, ParentID: 2, Name: "Grandchild"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := store.EditChannel(protocol.Channel{ID: 2, ParentID: 99, Name: "Child"}); err == nil {
		t.Fatal("accepted an unknown parent")
	}
	if err := store.EditChannel(protocol.Channel{ID: 1, ParentID: 3, Name: "Root"}); err == nil {
		t.Fatal("accepted a channel hierarchy cycle")
	}
}

func TestChannelAudioQualityPersistsWhileTemporaryChannelsDoNot(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	store, _, err := LoadOrCreate(path, []protocol.Channel{{ID: 1, Name: "Lobby"}})
	if err != nil {
		t.Fatal(err)
	}
	if err := store.EditChannel(protocol.Channel{ID: 1, Name: "Lobby", AudioQualityPolicy: "quality_10"}); err != nil {
		t.Fatal(err)
	}
	temporary, err := store.CreateChannel(protocol.Channel{Name: "Quick room", AudioQualityPolicy: "quality_5", Temporary: true})
	if err != nil {
		t.Fatal(err)
	}
	if !temporary.Temporary || temporary.AudioQualityPolicy != "quality_5" {
		t.Fatalf("temporary channel settings were not retained in memory: %+v", temporary)
	}
	if err := store.EditChannel(protocol.Channel{ID: temporary.ID, Name: "Renamed by legacy client"}); err != nil {
		t.Fatal(err)
	}
	for _, channel := range store.Channels() {
		if channel.ID == temporary.ID && (!channel.Temporary || channel.AudioQualityPolicy != "quality_5") {
			t.Fatalf("legacy edit erased new channel properties: %+v", channel)
		}
	}

	reloaded, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	channels := reloaded.Channels()
	if len(channels) != 1 || channels[0].ID != 1 || channels[0].AudioQualityPolicy != "quality_10" {
		t.Fatalf("unexpected channels after restart: %+v", channels)
	}
}
