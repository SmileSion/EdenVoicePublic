package clientupdates

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func fixture() Release {
	signature, e := os.ReadFile("../../../desktop/src-tauri/tests/fixtures/update.txt.sig")
	if e != nil {
		panic(e)
	}
	return Release{Platform: "windows-x86_64", Version: "0.7.6", URL: "https://example.com/releases/setup.exe", Signature: strings.TrimSpace(string(signature))}
}
func TestDraftPublishPausePersistence(t *testing.T) {
	p := filepath.Join(t.TempDir(), "updates.json")
	s, e := Open(p)
	if e != nil {
		t.Fatal(e)
	}
	r := fixture()
	d, e := s.Update("save", 1, r)
	if e != nil || len(s.Published()) != 0 {
		t.Fatal(d, e)
	}
	if _, e = s.Update("publish", 1, r); e == nil {
		t.Fatal("stale write accepted")
	}
	d, e = s.Update("publish", d.Revision, r)
	if e != nil || len(s.Published()) != 1 {
		t.Fatal(e)
	}
	r.Version = "0.8.0"
	d, e = s.Update("save", d.Revision, r)
	if e != nil || s.Published()[0].Version != "0.7.6" {
		t.Fatal("draft leaked")
	}
	s, e = Open(p)
	if e != nil || s.Published()[0].Version != "0.7.6" {
		t.Fatal("persistence", e)
	}
	_, e = s.Update("pause", d.Revision, r)
	if e != nil || len(s.Published()) != 0 {
		t.Fatal(e)
	}
	s, e = Open(p)
	if e != nil || len(s.Published()) != 0 {
		t.Fatal("pause persistence", e)
	}
}
func TestInvalidLinksAndInternalChecks(t *testing.T) {
	for _, url := range []string{"http://example.com/a", "https://example.com/a?token=secret", "https://user:pass@example.com/a"} {
		r := fixture()
		r.URL = url
		if Validate(r) == nil {
			t.Fatal(url)
		}
	}
	r := fixture()
	r.URL = "https://127.0.0.1/package.exe"
	if CheckURL(context.Background(), r) == nil {
		t.Fatal("internal URL accepted")
	}
	r = fixture()
	r.Signature = "bad"
	if Validate(r) == nil {
		t.Fatal("invalid signature")
	}
}
