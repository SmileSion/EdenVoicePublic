package webadmin

import (
	"edenvoice/server/internal/clientupdates"
	"os"
	"strings"
	"testing"
)

func TestUpdateAdminPermissionsAndDraftIsolation(t *testing.T) {
	f := setup(t)
	cookie, csrf := f.login(t, "operator")
	origin := f.s.origin.String()
	signature, e := os.ReadFile("../../../desktop/src-tauri/tests/fixtures/update.txt.sig")
	if e != nil {
		t.Fatal(e)
	}
	r := clientupdates.Release{Platform: "windows-x86_64", Version: "0.7.6", URL: "https://example.com/setup.exe", Signature: strings.TrimSpace(string(signature))}
	body := map[string]any{"action": "save", "revision": "1", "release": r}
	if v := f.request("POST", "updates", body, cookie, "", origin); v.Code != 403 {
		t.Fatal("csrf", v.Code)
	}
	if v := f.request("POST", "updates", body, cookie, csrf, origin); v.Code != 200 {
		t.Fatal(v.Body.String())
	}
	d, _ := f.s.control.WebClientUpdates("admin", "", 0, r)
	if d.Entries[r.Platform].Published != nil {
		t.Fatal("draft published")
	}
	viewer, vc := f.login(t, "viewer")
	if v := f.request("POST", "updates", body, viewer, vc, origin); v.Code != 403 {
		t.Fatal("readonly", v.Code)
	}
	if _, e := f.s.control.WebClientUpdates("user", "publish", d.Revision, r); e == nil {
		t.Fatal("ordinary member published")
	}
}
