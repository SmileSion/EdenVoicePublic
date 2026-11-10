package webadmin

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestAdminOverviewIncludesOptionalSessionBuild(t *testing.T) {
	f := setup(t)
	cookie, _ := f.login(t, "viewer")
	f.state.SetClientBuild(f.target, "0.8.1", "windows")
	r := f.request("GET", "overview", nil, cookie, "", "")
	if r.Code != 200 {
		t.Fatal(r.Code)
	}
	if !strings.Contains(r.Body.String(), `"client_version":"0.8.1"`) || !strings.Contains(r.Body.String(), `"client_platform":"windows"`) {
		t.Fatal("version missing from API")
	}
	var decoded map[string]any
	if e := json.Unmarshal(r.Body.Bytes(), &decoded); e != nil {
		t.Fatal(e)
	}
	r = f.request("GET", "overview", nil, nil, "", "")
	if r.Code != 401 {
		t.Fatal("anonymous access", r.Code)
	}
}
