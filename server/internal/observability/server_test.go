package observability

import (
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
)

func TestMetricsExposeMediaAndRuntimeSignals(t *testing.T) {
	cfg := config.Default()
	state := serverstate.New(cfg)
	session := state.AddSession("silent-user", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	relay := voice.New(cfg.VoiceAddr, state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	server := New(cfg.Observability.MetricsAddr, state, relay, slog.Default())
	request := httptest.NewRequest(http.MethodGet, "/metrics", nil)
	response := httptest.NewRecorder()
	server.metrics(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("unexpected status: %d", response.Code)
	}
	body := response.Body.String()
	for _, metric := range []string{
		"edenvoice_media_worker_queue_wait_max_seconds",
		"edenvoice_media_stale_queue_drops_total",
		"edenvoice_media_writer_queue_drops_total",
		"edenvoice_media_writer_stale_drops_total",
		"edenvoice_media_auth_failures_total",
		"edenvoice_process_cpu_seconds_total",
		"edenvoice_process_memory_alloc_bytes",
		"edenvoice_process_goroutines",
		"edenvoice_room_users{channel_id=\"1\"} 1",
	} {
		if !strings.Contains(body, metric) {
			t.Fatalf("missing metric %s in %q", metric, body)
		}
	}
}

func TestHealthRejectsMutationMethods(t *testing.T) {
	server := &Server{}
	response := httptest.NewRecorder()
	server.health(response, httptest.NewRequest(http.MethodPost, "/healthz", nil))
	if response.Code != http.StatusMethodNotAllowed {
		t.Fatalf("unexpected status: %d", response.Code)
	}
}
