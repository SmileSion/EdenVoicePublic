package webadmin

import (
	"edenvoice/server/internal/gamecatalog"
	"edenvoice/server/internal/protocol"
	"encoding/json"
	"testing"
)

func TestGameCatalogPermissionsAndPublication(t *testing.T) {
	f := setup(t)
	cookie, csrf := f.login(t, "operator")
	origin := f.s.origin.String()
	r := f.request("GET", "games", nil, cookie, "", "")
	if r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	var d gamecatalog.Document
	// Web IDs are strings; use the authoritative store revision for writes.
	d, _ = f.s.control.WebGameCatalog("admin", "", 0, nil)
	games := protocol.DefaultGames()
	games[0].Enabled = false
	body := map[string]any{"action": "save", "revision": "1", "games": games}
	if r = f.request("POST", "games", body, cookie, "", origin); r.Code != 403 {
		t.Fatal("missing csrf", r.Code)
	}
	if r = f.request("POST", "games", body, cookie, csrf, origin); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	if r = f.request("POST", "games", map[string]string{"action": "publish", "revision": "2"}, cookie, csrf, origin); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	d, _ = f.s.control.WebGameCatalog("admin", "", 0, nil)
	if d.Published.Games[0].Enabled || d.Published.Version != 2 {
		t.Fatal(d)
	}
	var state map[string]any
	b, _ := json.Marshal(f.state.Snapshot())
	json.Unmarshal(b, &state)
	if state["game_catalog_version"] != float64(2) {
		t.Fatal("catalog not distributed")
	}
	viewer, vc := f.login(t, "viewer")
	if r = f.request("POST", "games", body, viewer, vc, origin); r.Code != 403 {
		t.Fatal("readonly writes", r.Code)
	}
	if _, e := f.s.control.WebGameCatalog("user", "save", d.Revision, games); e == nil {
		t.Fatal("member writes")
	}
}
