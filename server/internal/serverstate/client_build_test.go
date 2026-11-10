package serverstate

import (
	"edenvoice/server/internal/config"
	"testing"
)

func TestClientBuildIsSessionScopedAndOptional(t *testing.T) {
	s := New(config.Default())
	a := s.AddAuthenticatedSession("A", "same-id", "USER", "", nil, nil, nil)
	b := s.AddAuthenticatedSession("B", "same-id", "USER", "", nil, nil, nil)
	s.SetClientBuild(a.ID, "0.8.1", "windows")
	first, _ := s.SessionByID(a.ID)
	second, _ := s.SessionByID(b.ID)
	if first.ClientVersion != "0.8.1" || first.ClientPlatform != "windows" || second.ClientVersion != "" {
		t.Fatal(first, second)
	}
	s.SetClientBuild(a.ID, "<script>", "<img>")
	first, _ = s.SessionByID(a.ID)
	if first.ClientVersion != "" || first.ClientPlatform != "" {
		t.Fatal("invalid metadata accepted")
	}
}
