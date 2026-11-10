package observability

import (
	"fmt"
	"log/slog"
	"net/http"
	"runtime"
	runtimemetrics "runtime/metrics"
	"sort"
	"time"

	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
)

type Server struct {
	addr    string
	state   *serverstate.State
	relay   *voice.Relay
	logger  *slog.Logger
	started time.Time
}

func New(addr string, state *serverstate.State, relay *voice.Relay, logger *slog.Logger) *Server {
	return &Server{addr: addr, state: state, relay: relay, logger: logger, started: time.Now()}
}

func (s *Server) ListenAndServe() error {
	mux := http.NewServeMux()
	mux.HandleFunc("/healthz", s.health)
	mux.HandleFunc("/metrics", s.metrics)
	s.logger.Info("observability server started", "addr", s.addr)
	server := &http.Server{
		Addr: s.addr, Handler: mux,
		ReadHeaderTimeout: 3 * time.Second,
		ReadTimeout:       5 * time.Second,
		WriteTimeout:      5 * time.Second,
		IdleTimeout:       30 * time.Second,
	}
	return server.ListenAndServe()
}

func (s *Server) health(w http.ResponseWriter, request *http.Request) {
	if request.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write([]byte("ok\n"))
}

func (s *Server) metrics(w http.ResponseWriter, request *http.Request) {
	if request.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	w.Header().Set("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
	media := s.relay.Metrics()
	runtimeStats := s.state.RuntimeStats()
	writeMetric(w, "edenvoice_online_users", float64(runtimeStats.OnlineUsers))
	writeMetric(w, "edenvoice_active_rooms", float64(runtimeStats.Rooms))
	writeMetric(w, "edenvoice_media_rx_packets_total", float64(media.RXPackets))
	writeMetric(w, "edenvoice_media_tx_packets_total", float64(media.TXPackets))
	writeMetric(w, "edenvoice_media_rx_bytes_total", float64(media.RXBytes))
	writeMetric(w, "edenvoice_media_tx_bytes_total", float64(media.TXBytes))
	writeMetric(w, "edenvoice_media_auth_failures_total", float64(media.AuthFailures))
	writeMetric(w, "edenvoice_media_write_failures_total", float64(media.WriteFailures))
	writeMetric(w, "edenvoice_media_worker_queue_depth", float64(media.WorkerQueueDepth))
	writeMetric(w, "edenvoice_media_worker_queue_high_water", float64(media.WorkerQueueHighWater))
	writeMetric(w, "edenvoice_media_worker_queue_drops_total", float64(media.WorkerQueueDrops))
	writeMetric(w, "edenvoice_media_stale_queue_drops_total", float64(media.StaleQueueDrops))
	writeMetric(w, "edenvoice_media_worker_queue_wait_max_seconds", float64(media.QueueWaitMaxMicros)/1_000_000)
	writeMetric(w, "edenvoice_media_writer_queue_depth", float64(media.WriterQueueDepth))
	writeMetric(w, "edenvoice_media_writer_queue_high_water", float64(media.WriterQueueHighWater))
	writeMetric(w, "edenvoice_media_writer_backpressure_total", float64(media.WriterBackpressureEvents))
	writeMetric(w, "edenvoice_media_writer_queue_drops_total", float64(media.WriterQueueDrops))
	writeMetric(w, "edenvoice_media_writer_stale_drops_total", float64(media.WriterStaleDrops))
	writeMetric(w, "edenvoice_media_writer_wait_max_seconds", float64(media.WriterWaitMaxMicros)/1_000_000)
	writeMetric(w, "edenvoice_process_uptime_seconds", time.Since(s.started).Seconds())
	writeMetric(w, "edenvoice_process_cpu_seconds_total", runtimeCPUSeconds())
	writeMetric(w, "edenvoice_process_goroutines", float64(runtime.NumGoroutine()))
	var memory runtime.MemStats
	runtime.ReadMemStats(&memory)
	writeMetric(w, "edenvoice_process_memory_alloc_bytes", float64(memory.Alloc))
	writeMetric(w, "edenvoice_process_memory_sys_bytes", float64(memory.Sys))
	writeMetric(w, "edenvoice_process_heap_inuse_bytes", float64(memory.HeapInuse))
	roomMetrics := make(map[uint64]voice.RoomMetricsSnapshot, len(media.Rooms))
	roomIDs := make([]uint64, 0, len(media.Rooms))
	for _, room := range media.Rooms {
		roomMetrics[room.ChannelID] = room
		roomIDs = append(roomIDs, room.ChannelID)
	}
	for _, channelID := range s.state.ActiveRooms() {
		if _, exists := roomMetrics[channelID]; !exists {
			roomMetrics[channelID] = voice.RoomMetricsSnapshot{ChannelID: channelID}
			roomIDs = append(roomIDs, channelID)
		}
	}
	sort.Slice(roomIDs, func(i, j int) bool { return roomIDs[i] < roomIDs[j] })
	for _, channelID := range roomIDs {
		room := roomMetrics[channelID]
		labels := fmt.Sprintf("channel_id=\"%d\"", channelID)
		writeLabeledMetric(w, "edenvoice_room_users", labels, float64(s.state.RoomSize(channelID)))
		writeLabeledMetric(w, "edenvoice_room_media_rx_packets_total", labels, float64(room.RXPackets))
		writeLabeledMetric(w, "edenvoice_room_media_tx_packets_total", labels, float64(room.TXPackets))
		writeLabeledMetric(w, "edenvoice_room_media_rx_packets_per_second", labels, room.RXPacketsPerSecond)
		writeLabeledMetric(w, "edenvoice_room_media_tx_packets_per_second", labels, room.TXPacketsPerSecond)
	}
}

func runtimeCPUSeconds() float64 {
	samples := []runtimemetrics.Sample{
		{Name: "/cpu/classes/user:cpu-seconds"},
		{Name: "/cpu/classes/gc/total:cpu-seconds"},
		{Name: "/cpu/classes/scavenge/total:cpu-seconds"},
	}
	runtimemetrics.Read(samples)
	var total float64
	for _, sample := range samples {
		if sample.Value.Kind() == runtimemetrics.KindFloat64 {
			total += sample.Value.Float64()
		}
	}
	return total
}

func writeMetric(w http.ResponseWriter, name string, value float64) {
	_, _ = fmt.Fprintf(w, "%s %g\n", name, value)
}

func writeLabeledMetric(w http.ResponseWriter, name, labels string, value float64) {
	_, _ = fmt.Fprintf(w, "%s{%s} %g\n", name, labels, value)
}
