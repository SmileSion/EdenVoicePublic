package screenshare_test

import (
	"context"
	. "edenvoice/server/internal/screenshare"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"testing"
	"time"
)

// Optional real-browser codec test. Uses generated pixels/tones rather than OS
// desktop capture: permission, source picker and native WebViews need separate QA.
func TestBrowserDecodesScreenAndAudio(t *testing.T) {
	if os.Getenv("EDEN_SCREEN_BROWSER") != "1" {
		t.Skip("set EDEN_SCREEN_BROWSER=1 with PLAYWRIGHT_MODULE and CHROME_EXECUTABLE")
	}
	cfg := DefaultConfig()
	cfg.Enabled = true
	cfg.PublicIP = "127.0.0.1"
	portProbe, err := net.ListenUDP("udp4", &net.UDPAddr{Port: 0})
	if err != nil {
		t.Fatal(err)
	}
	cfg.UDPPort = uint16(portProbe.LocalAddr().(*net.UDPAddr).Port)
	_ = portProbe.Close()
	m, err := New(cfg, Hooks{Allowed: func(id, room uint64) bool { return (id == 1 || id == 2) && room == 7 }, Changed: func(room uint64, reason string) {
		if reason != "" {
			t.Log("share ended:", reason)
		}
	}})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(m.Close)
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "GET" {
			w.Header().Set("Content-Type", "text/html")
			_, _ = w.Write([]byte("<!doctype html><title>Screen relay test</title><body></body>"))
			return
		}
		var req struct {
			Paused    *bool    `json:"paused,omitempty"`
			RequestID string   `json:"request_id"`
			Action    string   `json:"action"`
			ShareID   string   `json:"share_id"`
			Settings  Settings `json:"settings"`
			SDP       string   `json:"sdp"`
		}
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 65536)).Decode(&req) != nil {
			http.Error(w, "invalid request", 400)
			return
		}
		id := uint64(1)
		if r.URL.Query().Get("viewer") == "1" {
			id = 2
		}
		state := m.Status(7)
		var sdp string
		var err error
		switch req.Action {
		case "publish":
			state, sdp, err = m.Publish(r.Context(), id, 7, req.Settings, req.SDP)
		case "subscribe":
			state, sdp, err = m.Subscribe(r.Context(), id, 7, req.ShareID, req.SDP)
		case "pause":
			if req.Paused == nil {
				err = errors.New("privacy pause requires an explicit paused boolean")
			} else {
				err = m.Pause(id, 7, req.ShareID, *req.Paused)
			}
			state = m.Status(7)
		case "stop":
			err = m.Stop(id, 7, req.ShareID, false)
			state = m.Status(7)
		case "cancel":
			m.RemoveSession(id)
			state = m.Status(7)
		case "leave":
			m.Leave(id, 7, req.ShareID)
			state = m.Status(7)
		}
		data, _ := json.Marshal(state)
		var response map[string]any
		_ = json.Unmarshal(data, &response)
		response["revision"] = strconv.FormatUint(state.Revision, 10)
		response["channel_id"] = "7"
		response["host_id"] = "1"
		response["server_address"] = "fixture"
		response["self_session_id"] = "1"
		if id == 2 {
			response["self_session_id"] = "2"
		}
		response["action"] = req.Action
		response["request_id"] = req.RequestID
		response["sdp"] = sdp
		if err != nil {
			response["error"] = err.Error()
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(response)
	})
	server := httptest.NewServer(handler)
	defer server.Close()
	root, err := filepath.Abs("../../..")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 75*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "node", "scripts/screen-share-browser.test.mjs", server.URL)
	cmd.Dir = filepath.Join(root, "desktop")
	stopVoice := concurrentVoice(t)
	output, err := cmd.CombinedOutput()
	stopVoice()
	t.Log(string(output))
	if err != nil {
		t.Fatal(err)
	}
	if m.Status(7).ShareID != "" {
		t.Fatal("browser stop retained sharing room")
	}
}
