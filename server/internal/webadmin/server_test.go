package webadmin

import (
	"bytes"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/control"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
)

const testPassword = "Console-test-2026!"

type fixture struct {
	s       *Server
	store   *administration.Store
	state   *serverstate.State
	handler http.Handler
	target  uint64
}
type receiver struct{ messages []protocol.Envelope }

func (r *receiver) Send(e protocol.Envelope) error { r.messages = append(r.messages, e); return nil }
func setup(t *testing.T) *fixture {
	t.Helper()
	cfg := config.Default()
	state := serverstate.New(cfg)
	dir := t.TempDir()
	store, token, err := administration.LoadOrCreate(filepath.Join(dir, "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"master", "admin", "user"} {
		if _, err = store.Admit(id, "DO_NOT_EXPOSE_PUBLIC_KEY", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err = store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	if err = store.SetServerRole("admin", administration.RoleAdmin); err != nil {
		t.Fatal(err)
	}
	target := state.AddAuthenticatedSession("User", "user", "USER", "EPHEMERAL_SECRET", &receiver{}, nil, nil)
	if err = state.Join(target.ID, 1); err != nil {
		t.Fatal(err)
	}
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	ctrl := control.NewSecure("", "", state, logger, nil, nil, false, store)
	relay := voice.New("", state, logger)
	accounts, err := OpenAccounts(filepath.Join(dir, "web.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"owner", "operator", "viewer"} {
		id := "admin"
		if name == "owner" {
			id = "master"
		}
		if err = accounts.Put(name, id, testPassword, name == "viewer", false); err != nil {
			t.Fatal(err)
		}
	}
	s, err := New(config.WebAdmin{Enabled: true, ListenAddr: "127.0.0.1:9101"}, ctrl, relay, accounts, logger)
	if err != nil {
		t.Fatal(err)
	}
	return &fixture{s: s, store: store, state: state, handler: s.Handler(), target: target.ID}
}
func (f *fixture) request(method, path string, body any, cookie *http.Cookie, csrf, origin string) *httptest.ResponseRecorder {
	var raw []byte
	if body != nil {
		raw, _ = json.Marshal(body)
	}
	r := httptest.NewRequest(method, "http://127.0.0.1:9101/api/"+path, bytes.NewReader(raw))
	r.Header.Set("Content-Type", "application/json")
	if origin != "" {
		r.Header.Set("Origin", origin)
	}
	r.Header.Set("X-CSRF-Token", csrf)
	if cookie != nil {
		r.AddCookie(cookie)
	}
	w := httptest.NewRecorder()
	f.handler.ServeHTTP(w, r)
	return w
}
func (f *fixture) login(t *testing.T, username string) (*http.Cookie, string) {
	t.Helper()
	w := f.request("POST", "login", map[string]string{"username": username, "password": testPassword}, nil, "", f.s.origin.String())
	if w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	var result map[string]string
	json.Unmarshal(w.Body.Bytes(), &result)
	cookie := w.Result().Cookies()[0]
	if !cookie.HttpOnly || cookie.SameSite != http.SameSiteStrictMode {
		t.Fatal("insecure session cookie", cookie)
	}
	return cookie, result["csrf"]
}
func TestConsoleAuthenticationAuthorizationAndPersistence(t *testing.T) {
	f := setup(t)
	if w := f.request("GET", "overview", nil, nil, "", ""); w.Code != 401 {
		t.Fatal("anonymous overview", w.Code)
	}
	if w := f.request("POST", "login", map[string]string{"username": "owner", "password": testPassword}, nil, "", "https://attacker.example"); w.Code != 403 {
		t.Fatal("cross-origin login", w.Code)
	}
	cookie, csrf := f.login(t, "owner")
	action := map[string]any{"action": "create_channel", "payload": map[string]any{"channel": map[string]any{"name": "Console room", "parent_id": "0", "audio_quality_policy": "quality_8", "network_policy": "auto"}}}
	if w := f.request("POST", "command", action, cookie, "", f.s.origin.String()); w.Code != 403 {
		t.Fatal("missing CSRF allowed", w.Code)
	}
	if w := f.request("POST", "command", action, cookie, csrf, "http://localhost:9999"); w.Code != 403 {
		t.Fatal("wrong origin allowed", w.Code)
	}
	before := len(f.state.Snapshot().Users)
	if w := f.request("POST", "command", action, cookie, csrf, f.s.origin.String()); w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	if len(f.state.Snapshot().Users) != before {
		t.Fatal("web management created a voice session")
	}
	if len(f.store.Channels()) != 4 {
		t.Fatal("channel not persisted")
	}
	reloaded, _, err := administration.LoadOrCreate(f.storePath(), nil)
	if err != nil || len(reloaded.Channels()) != 4 {
		t.Fatal("channel did not survive reload", err)
	}
	w := f.request("GET", "overview", nil, cookie, "", "")
	if w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	raw := w.Body.String()
	for _, secret := range []string{"DO_NOT_EXPOSE_PUBLIC_KEY", "EPHEMERAL_SECRET", "password_hash", testPassword} {
		if strings.Contains(raw, secret) {
			t.Fatal("secret disclosed", secret)
		}
	}
	if !strings.Contains(raw, `"session_id":"`+strconv.FormatUint(f.target, 10)+`"`) {
		t.Fatal("session ID lost precision")
	}
	for _, step := range []struct {
		action  string
		payload map[string]any
	}{
		{"admin_mute", map[string]any{"client_id": "user", "session_id": strconv.FormatUint(f.target, 10), "enabled": true}},
		{"move_user", map[string]any{"client_id": "user", "session_id": strconv.FormatUint(f.target, 10), "channel_id": "2"}},
		{"set_room_network_policy", map[string]any{"channel_id": "2", "policy": "force_sfu"}},
		{"set_channel_admin", map[string]any{"channel_id": "2", "client_id": "user", "enabled": true}},
	} {
		if w := f.request("POST", "command", map[string]any{"action": step.action, "payload": step.payload}, cookie, csrf, f.s.origin.String()); w.Code != 200 {
			t.Fatal(step.action, w.Code, w.Body.String())
		}
	}
	u, _ := f.state.SessionByID(f.target)
	if !u.AdminMuted || u.ChannelID != 2 {
		t.Fatal("session management not applied", u)
	}
	if f.state.RoomNetworkPolicy(2) != "force_sfu" || f.store.ChannelRole(2, "user") != administration.RoleChannelAdmin {
		t.Fatal("room settings not applied")
	}
	op, opCSRF := f.login(t, "operator")
	denied := map[string]any{"action": "set_server_role", "payload": map[string]any{"client_id": "user", "role": "ADMIN"}}
	if w := f.request("POST", "command", denied, op, opCSRF, f.s.origin.String()); w.Code != 403 {
		t.Fatal("admin escalated roles", w.Code)
	}
	viewer, vCSRF := f.login(t, "viewer")
	if w := f.request("POST", "command", action, viewer, vCSRF, f.s.origin.String()); w.Code != 403 {
		t.Fatal("read-only mutation", w.Code)
	}
	if w := f.request("GET", "accounts", nil, viewer, "", ""); w.Code != 403 {
		t.Fatal("read-only account access", w.Code)
	}
	if w := f.request("GET", "overview", nil, viewer, "", ""); w.Code != 200 {
		t.Fatal("viewer cannot read", w.Code)
	}
	if err := f.store.SetServerRole("admin", administration.RoleUser); err != nil {
		t.Fatal(err)
	}
	if w := f.request("GET", "overview", nil, op, "", ""); w.Code != 401 {
		t.Fatal("revoked admin session remains usable", w.Code)
	}
	if w := f.request("POST", "logout", map[string]any{}, cookie, csrf, f.s.origin.String()); w.Code != 200 {
		t.Fatal(w.Code)
	}
	if w := f.request("GET", "overview", nil, cookie, "", ""); w.Code != 401 {
		t.Fatal("logout session reused", w.Code)
	}
}
func (f *fixture) storePath() string {
	return filepath.Join(filepath.Dir(f.s.accounts.path), "admin.json")
}
func TestConsoleAccountChangesAndExpiry(t *testing.T) {
	f := setup(t)
	cookie, csrf := f.login(t, "owner")
	req := map[string]any{"username": "new-user", "client_id": "user", "password": testPassword}
	if w := f.request("POST", "accounts", req, cookie, csrf, f.s.origin.String()); w.Code != 400 {
		t.Fatal("non-admin account accepted", w.Code)
	}
	req["client_id"] = "admin"
	if w := f.request("POST", "accounts", req, cookie, csrf, f.s.origin.String()); w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	other, _ := f.login(t, "new-user")
	req["disabled"] = true
	req["password"] = ""
	if w := f.request("POST", "accounts", req, cookie, csrf, f.s.origin.String()); w.Code != 200 {
		t.Fatal(w.Code)
	}
	if w := f.request("GET", "overview", nil, other, "", ""); w.Code != 401 {
		t.Fatal("disabled account token accepted", w.Code)
	}
	if w := f.request("POST", "password", map[string]string{"current": testPassword, "password": ""}, cookie, csrf, f.s.origin.String()); w.Code != 400 {
		t.Fatal("empty password accepted", w.Code)
	}
	if w := f.request("POST", "password", map[string]string{"current": testPassword, "password": "New-console-pass-2026!"}, cookie, csrf, f.s.origin.String()); w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	if w := f.request("GET", "overview", nil, cookie, "", ""); w.Code != 401 {
		t.Fatal("old token survived password change", w.Code)
	}
	raw, err := os.ReadFile(f.s.accounts.path)
	if err != nil || bytes.Contains(raw, []byte(testPassword)) {
		t.Fatal("plaintext password persisted")
	}
	if info, err := os.Stat(f.s.accounts.path); err != nil || info.Mode().Perm() != 0600 {
		t.Fatal("account file permissions", err)
	}
	op, _ := f.login(t, "operator")
	f.s.mu.Lock()
	for k, v := range f.s.sessions {
		v.Seen = time.Now().Add(-time.Hour)
		f.s.sessions[k] = v
	}
	f.s.mu.Unlock()
	if w := f.request("GET", "session", nil, op, "", ""); w.Code != 401 {
		t.Fatal("expired session accepted", w.Code)
	}
}
func TestConsoleBoundariesAndRateLimits(t *testing.T) {
	f := setup(t)
	r := httptest.NewRequest("GET", "http://evil.example/api/session", nil)
	w := httptest.NewRecorder()
	f.handler.ServeHTTP(w, r)
	if w.Code != 421 {
		t.Fatal("DNS rebinding host accepted")
	}
	for i := 0; i < 10; i++ {
		f.s.rate("login-ip:198.51.100.2", 10, time.Now())
	}
	r = httptest.NewRequest("POST", "http://127.0.0.1:9101/api/login", strings.NewReader(`{}`))
	r.RemoteAddr = "198.51.100.2:1234"
	r.Header.Set("Origin", f.s.origin.String())
	r.Header.Set("Content-Type", "application/json")
	w = httptest.NewRecorder()
	f.handler.ServeHTTP(w, r)
	if w.Code != 429 {
		t.Fatal("login rate limit failed", w.Code)
	}
	cookie, csrf := f.login(t, "owner")
	if w := f.request("POST", "command", map[string]any{"action": "provision_master", "payload": map[string]any{"token": "ignored"}}, cookie, csrf, f.s.origin.String()); w.Code != 409 {
		t.Fatal("unlisted command accepted", w.Code)
	}
	if w := f.request("POST", "command", map[string]any{"action": "move_user", "payload": map[string]any{"client_id": "master", "session_id": strconv.FormatUint(f.target, 10), "channel_id": "2"}}, cookie, csrf, f.s.origin.String()); w.Code != 409 {
		t.Fatal("mismatched identity/session accepted")
	}
	if w := f.request("POST", "command", map[string]any{"action": "create_channel", "payload": map[string]any{"channel": map[string]any{"name": "empty temporary", "temporary": true}}}, cookie, csrf, f.s.origin.String()); w.Code != 409 {
		t.Fatal("web created orphan temporary room")
	}
	first := f.s.performance()
	second := f.s.performance()
	if len(first) != len(second) || !first[0].At.Equal(second[0].At) {
		t.Fatal("metrics not cached")
	}
}
