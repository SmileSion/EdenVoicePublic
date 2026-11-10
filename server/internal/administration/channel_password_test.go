package administration

import (
	"bytes"
	"edenvoice/server/internal/protocol"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestChannelPasswordsPersistWithoutDisclosure(t *testing.T) {
	path := filepath.Join(t.TempDir(), "admin.json")
	s, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	c, err := s.CreateChannelWithPassword(protocol.Channel{Name: "private"}, "secret-room-123")
	if err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{"", "wrong"} {
		if s.CheckChannelPassword(c.ID, p) == nil {
			t.Fatal("accepted wrong password")
		}
	}
	if err = s.CheckChannelPassword(c.ID, "secret-room-123"); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(path)
	if bytes.Contains(raw, []byte("secret-room-123")) {
		t.Fatal("plaintext saved")
	}
	public, _ := json.Marshal(s.Channels())
	if bytes.Contains(public, []byte("password")) {
		t.Fatal("hash disclosed")
	}
	restored, _, err := LoadOrCreate(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err = restored.CheckChannelPassword(c.ID, "secret-room-123"); err != nil {
		t.Fatal(err)
	}
	if err = restored.SetChannelPassword(c.ID, ""); err != nil {
		t.Fatal(err)
	}
	if len(restored.LockedChannelIDs()) != 0 || restored.CheckChannelPassword(c.ID, "") != nil {
		t.Fatal("password not removed")
	}
}
