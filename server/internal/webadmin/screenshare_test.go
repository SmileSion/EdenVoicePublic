package webadmin

import (
	"edenvoice/server/internal/screenshare"
	"net"
	"os"
	"path/filepath"
	"testing"
)

func TestScreenSharingPolicyPermissionsPersistenceAndConflict(t *testing.T) {
	f := setup(t)
	cfg := screenshare.DefaultConfig()
	cfg.UDPPort = screenTestUDPPort(t)
	path := filepath.Join(t.TempDir(), "screen-sharing.json")
	if err := f.s.control.ConfigureScreenSharing(cfg, path); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(f.s.control.CloseScreenSharing)
	cookie, csrf := f.login(t, "operator")
	origin := f.s.origin.String()
	if r := f.request("GET", "screen-sharing", nil, cookie, "", ""); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	cfg.Policy.Enabled = true
	cfg.Policy.MaxFPS = 15
	body := map[string]any{"revision": "1", "policy": cfg.Policy}
	if r := f.request("POST", "screen-sharing", body, cookie, "", origin); r.Code != 403 {
		t.Fatal("missing csrf", r.Code)
	}
	if r := f.request("POST", "screen-sharing", body, cookie, csrf, origin); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	if r := f.request("POST", "screen-sharing", body, cookie, csrf, origin); r.Code != 409 {
		t.Fatal("stale write accepted", r.Code)
	}
	saved, err := screenshare.LoadPolicy(path, screenshare.DefaultConfig().Policy)
	if err != nil || saved.Revision != 2 || saved.Policy != cfg.Policy {
		t.Fatal(saved, err)
	}
	if f.s.control.ScreenSharePolicy() != cfg.Policy {
		t.Fatal("runtime policy not applied")
	}
	viewer, vc := f.login(t, "viewer")
	if r := f.request("POST", "screen-sharing", body, viewer, vc, origin); r.Code != 403 {
		t.Fatal("readonly write", r.Code)
	}
	if _, err := f.s.control.WebScreenSharing("user", nil); err == nil {
		t.Fatal("ordinary member read policy admin endpoint")
	}
	invalid := cfg.Policy
	invalid.MaxFPS = 0
	if r := f.request("POST", "screen-sharing", map[string]any{"revision": "2", "policy": invalid}, cookie, csrf, origin); r.Code != 409 {
		t.Fatal("invalid policy accepted", r.Code)
	}
	after, err := screenshare.LoadPolicy(path, screenshare.DefaultConfig().Policy)
	if err != nil || after != saved {
		t.Fatal("invalid update changed persisted policy")
	}
}

func TestScreenPolicySaveFailureLeavesRuntimeUnchanged(t *testing.T) {
	f := setup(t)
	cfg := screenshare.DefaultConfig()
	cfg.UDPPort = screenTestUDPPort(t)
	path := filepath.Join(t.TempDir(), "policy.json")
	if err := f.s.control.ConfigureScreenSharing(cfg, path); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(f.s.control.CloseScreenSharing)
	if err := os.Mkdir(path, 0700); err != nil {
		t.Fatal(err)
	}
	changed := cfg.Policy
	changed.Enabled = true
	if _, err := f.s.control.WebScreenSharing("admin", &screenshare.PolicyDocument{Revision: 1, Policy: changed}); err == nil {
		t.Fatal("expected storage failure")
	}
	if f.s.control.ScreenSharePolicy() != cfg.Policy {
		t.Fatal("storage failure changed runtime policy")
	}
	doc, err := f.s.control.WebScreenSharing("admin", nil)
	if err != nil || doc.Revision != 1 {
		t.Fatal("failed save advanced revision", doc, err)
	}
}

func screenTestUDPPort(t *testing.T) uint16 {
	t.Helper()
	c, err := net.ListenUDP("udp4", &net.UDPAddr{Port: 0})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	return uint16(c.LocalAddr().(*net.UDPAddr).Port)
}

func TestScreenPolicyBindFailureDoesNotPersistOrAdvanceRevision(t *testing.T) {
	f := setup(t)
	cfg := screenshare.DefaultConfig()
	occupied, err := net.ListenUDP("udp4", &net.UDPAddr{Port: 0})
	if err != nil {
		t.Fatal(err)
	}
	defer occupied.Close()
	cfg.UDPPort = uint16(occupied.LocalAddr().(*net.UDPAddr).Port)
	path := filepath.Join(t.TempDir(), "policy.json")
	original := screenshare.PolicyDocument{Revision: 1, Policy: cfg.Policy}
	if err = screenshare.SavePolicy(path, original); err != nil {
		t.Fatal(err)
	}
	if err = f.s.control.ConfigureScreenSharing(cfg, path); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(f.s.control.CloseScreenSharing)
	enabled := cfg.Policy
	enabled.Enabled = true
	if _, err = f.s.control.WebScreenSharing("admin", &screenshare.PolicyDocument{Revision: 1, Policy: enabled}); err == nil {
		t.Fatal("occupied UDP port was accepted")
	}
	saved, err := screenshare.LoadPolicy(path, cfg.Policy)
	if err != nil || saved != original {
		t.Fatal("bind failure persisted policy", saved, err)
	}
	current, err := f.s.control.WebScreenSharing("admin", nil)
	if err != nil || current != original || f.s.control.ScreenSharePolicy() != cfg.Policy {
		t.Fatal("bind failure changed runtime/revision", current, err)
	}
	if err = occupied.Close(); err != nil {
		t.Fatal(err)
	}
	next, err := f.s.control.WebScreenSharing("admin", &screenshare.PolicyDocument{Revision: 1, Policy: enabled})
	if err != nil || next.Revision != 2 || !next.Policy.Enabled {
		t.Fatal("retry enable failed", next, err)
	}
}
