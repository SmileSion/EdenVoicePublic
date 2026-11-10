package voice

import (
	"fmt"
	"log/slog"
	"net"
	"runtime"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
)

const (
	defaultWorkerQueueSize  = 64
	defaultMaxQueueDelay    = 40 * time.Millisecond
	defaultWriterQueueSize  = 256
	defaultWriteBatchSize   = 64
	maxAutomaticWorkers     = 8
	candidateNoticeInterval = 500 * time.Millisecond
)

type Options struct {
	WorkerCount     int
	WorkerQueueSize int
	MaxQueueDelay   time.Duration
	WriterQueueSize int
	WriteBatchSize  int
}

func (options Options) normalized() Options {
	if options.WorkerCount <= 0 {
		options.WorkerCount = min(runtime.GOMAXPROCS(0), maxAutomaticWorkers)
	}
	if options.WorkerQueueSize <= 0 {
		options.WorkerQueueSize = defaultWorkerQueueSize
	}
	if options.MaxQueueDelay <= 0 {
		options.MaxQueueDelay = defaultMaxQueueDelay
	}
	if options.WriterQueueSize <= 0 {
		options.WriterQueueSize = defaultWriterQueueSize
	}
	if options.WriteBatchSize <= 0 {
		options.WriteBatchSize = defaultWriteBatchSize
	}
	return options
}

type Relay struct {
	addr                    string
	state                   *serverstate.State
	logger                  *slog.Logger
	options                 Options
	bufferPool              sync.Pool
	metrics                 mediaMetrics
	invalidPacketLogs       diagnosticLimiter
	invalidRegistrationLogs diagnosticLimiter
	invalidSourceLogs       diagnosticLimiter
	writeFailureLogs        diagnosticLimiter
}

func New(addr string, state *serverstate.State, logger *slog.Logger, configured ...Options) *Relay {
	options := Options{}.normalized()
	if len(configured) > 0 {
		options = configured[0].normalized()
	}
	relay := &Relay{
		addr: addr, state: state, logger: logger,
		options:           options,
		invalidPacketLogs: newDiagnosticLimiter(10*time.Second, 3), invalidRegistrationLogs: newDiagnosticLimiter(10*time.Second, 3),
		invalidSourceLogs: newDiagnosticLimiter(10*time.Second, 3), writeFailureLogs: newDiagnosticLimiter(10*time.Second, 3),
	}
	relay.bufferPool.New = func() any { return make([]byte, protocol.MaxVoicePacketSize+1) }
	return relay
}

type diagnosticLimiter struct {
	mu          sync.Mutex
	interval    time.Duration
	burst       uint64
	windowStart time.Time
	emitted     uint64
	suppressed  uint64
}

func newDiagnosticLimiter(interval time.Duration, burst uint64) diagnosticLimiter {
	return diagnosticLimiter{interval: interval, burst: burst}
}

func (l *diagnosticLimiter) allow(now time.Time) (bool, uint64) {
	l.mu.Lock()
	defer l.mu.Unlock()
	var suppressed uint64
	if l.windowStart.IsZero() || now.Sub(l.windowStart) >= l.interval {
		suppressed = l.suppressed
		l.windowStart = now
		l.emitted = 0
		l.suppressed = 0
	}
	if l.emitted >= l.burst {
		l.suppressed++
		return false, 0
	}
	l.emitted++
	return true, suppressed
}

func (r *Relay) warn(limiter *diagnosticLimiter, message string, args ...any) {
	allowed, suppressed := limiter.allow(time.Now())
	if !allowed {
		return
	}
	if suppressed > 0 {
		args = append(args, "suppressed_since_last_sample", suppressed)
	}
	r.logger.Warn(message, args...)
}

func (r *Relay) ListenAndServe() error {
	network, err := udpNetwork(r.addr)
	if err != nil {
		return err
	}
	addr, err := net.ResolveUDPAddr(network, r.addr)
	if err != nil {
		return err
	}
	conn, err := net.ListenUDP(network, addr)
	if err != nil {
		return err
	}
	defer conn.Close()
	r.logger.Info("voice relay started", "addr", conn.LocalAddr(), "workers", r.options.WorkerCount, "worker_queue_size", r.options.WorkerQueueSize, "max_queue_delay", r.options.MaxQueueDelay, "writer_queue_size", r.options.WriterQueueSize, "write_batch_size", r.options.WriteBatchSize)
	go r.logMetrics()
	return r.Serve(conn)
}

func udpNetwork(address string) (string, error) {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return "", fmt.Errorf("invalid voice address: %w", err)
	}
	ip := net.ParseIP(strings.Trim(host, "[]"))
	if ip == nil {
		return "udp", nil
	}
	if ip.To4() != nil {
		return "udp4", nil
	}
	return "udp6", nil
}

func (r *Relay) Serve(conn *net.UDPConn) error {
	writer := r.startWriter(conn)
	workers := r.startWorkers(writer)
	defer func() {
		r.stopWorkers(workers)
		writer.stop()
	}()
	for {
		buffer := r.getBuffer()
		n, source, readErr := conn.ReadFromUDP(buffer)
		if readErr != nil {
			r.putBuffer(buffer)
			return readErr
		}
		packet, parseErr := protocol.ParseVoicePacket(buffer[:n])
		if parseErr != nil {
			r.metrics.invalidPackets.Add(1)
			r.warn(&r.invalidPacketLogs, "invalid voice packet", "source", source, "error", parseErr)
			r.putBuffer(buffer)
			continue
		}
		if packet.Type == protocol.VoiceTypeProbeAck {
			r.putBuffer(buffer)
			continue
		}
		r.enqueue(workers, relayJob{packet: packet, source: source, buffer: buffer, length: n, receivedAt: time.Now()})
	}
}

func (r *Relay) handlePacket(writer *relayWriter, job relayJob) {
	packet := job.packet
	if packet.Type == protocol.VoiceTypeRegister {
		r.handleRegistration(job)
		return
	}
	if validation := r.state.CheckUDPSource(packet.SessionID, job.source); validation != serverstate.UDPSourceValid {
		// The control session is removed before its audio sender necessarily observes
		// shutdown. Already-queued datagrams are a normal disconnect race, not an
		// unauthenticated source attempt, so keep them out of security diagnostics.
		if validation == serverstate.UDPSourceUnknownSession {
			return
		}
		reason := "source_mismatch"
		if validation == serverstate.UDPSourceUnregistered {
			reason = "not_registered"
		}
		r.metrics.invalidSources.Add(1)
		r.warn(&r.invalidSourceLogs, "invalid voice source", "source", job.source, "session_id", packet.SessionID, "reason", reason)
		return
	}
	if packet.Type == protocol.VoiceTypeProbe {
		plaintext := packet.Payload
		inbound, outbound, secure := r.state.MediaSecurity(packet.SessionID)
		if secure {
			var err error
			plaintext, err = inbound.Open(job.buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload)
			if err != nil {
				r.metrics.authFailures.Add(1)
				return
			}
		}
		if len(plaintext) != 8 {
			return
		}
		packet.Type = protocol.VoiceTypeProbeAck
		packet.Payload = plaintext
		if secure {
			var err error
			packet, err = outbound.SealPacket(packet, plaintext)
			if err != nil {
				return
			}
		}
		encoded, err := packet.MarshalBinary()
		if err != nil {
			return
		}
		writer.enqueue(outboundBatch{datagrams: []outboundDatagram{{
			payload: encoded,
			target:  job.source,
		}}})
		return
	}
	if !r.state.CanSpeak(packet.SessionID) {
		r.metrics.mutedDrops.Add(1)
		return
	}
	plaintext := packet.Payload
	if inbound, _, secure := r.state.MediaSecurity(packet.SessionID); secure {
		var err error
		plaintext, err = inbound.Open(job.buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload)
		if err != nil {
			r.metrics.authFailures.Add(1)
			return
		}
	}
	r.metrics.rxPackets.Add(1)
	r.metrics.rxBytes.Add(uint64(job.length))
	channelID := r.state.ChannelOf(packet.SessionID)
	r.metrics.observeRoomRX(channelID, job.length)
	targets := r.state.VoiceTargets(packet.SessionID)
	if len(targets) == 0 {
		return
	}
	batch := outboundBatch{datagrams: make([]outboundDatagram, 0, len(targets))}

	for _, target := range targets {
		outgoing := job.buffer[:job.length]
		buffer := r.getBuffer()
		if target.MediaOutbound != nil {
			sealed, err := target.MediaOutbound.SealPacket(packet, plaintext)
			if err != nil {
				r.putBuffer(buffer)
				continue
			}
			outgoing, err = sealed.AppendBinary(buffer[:0])
			if err != nil {
				r.putBuffer(buffer)
				continue
			}
		} else {
			outgoing = append(buffer[:0], outgoing...)
		}
		batch.datagrams = append(batch.datagrams, outboundDatagram{
			payload: outgoing, buffer: buffer, target: target.Address, channelID: channelID,
			expiresAt: job.receivedAt.Add(r.options.MaxQueueDelay),
		})
	}
	if len(batch.datagrams) > 0 {
		writer.enqueue(batch)
	}
}

func (r *Relay) handleRegistration(job relayJob) {
	packet := job.packet
	if inbound, _, secure := r.state.MediaSecurity(packet.SessionID); secure {
		if _, err := inbound.Open(job.buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload); err != nil {
			r.metrics.authFailures.Add(1)
			return
		}
	} else if len(packet.Payload) != 0 {
		r.metrics.invalidRegistrations.Add(1)
		return
	}
	changed, err := r.state.RefreshUDP(packet.SessionID, job.source)
	if err != nil {
		r.metrics.invalidRegistrations.Add(1)
		r.warn(&r.invalidRegistrationLogs, "invalid udp registration", "source", job.source)
		return
	}
	if !changed {
		return
	}
	channelID := r.state.ChannelOf(packet.SessionID)
	if !r.state.PeerCandidateFanoutAllowed(channelID) {
		return
	}
	// Endpoint binding is updated for every authenticated migration, but a
	// broken NAT/client can otherwise turn rapid port churn into a room-wide
	// control-message storm. Periodic route reconciliation provides the same
	// SFU safety while peer-candidate announcements are coalesced here.
	if !r.state.ClaimCandidateNotice(packet.SessionID, time.Now(), candidateNoticeInterval) {
		return
	}
	r.logger.Info("udp registered", "session_id", packet.SessionID, "source", job.source)
	if self, ok := r.state.CandidateSnapshot(packet.SessionID); ok {
		if msg, err := protocol.NewEnvelope(protocol.TypePeerCandidates, self); err == nil {
			r.state.BroadcastRoom(r.state.ChannelOf(packet.SessionID), msg, packet.SessionID)
		}
	}
	for _, peer := range r.state.PeerCandidates(packet.SessionID) {
		if msg, err := protocol.NewEnvelope(protocol.TypePeerCandidates, peer); err == nil {
			r.state.SendTo(packet.SessionID, msg)
		}
	}
	if channelID != 0 {
		route := r.state.EvaluateRoom(channelID)
		if msg, err := protocol.NewEnvelope(protocol.TypeRouteUpdate, route); err == nil {
			r.state.BroadcastRoom(channelID, msg, 0)
		}
	}
}

func (r *Relay) Metrics() MetricsSnapshot { return r.metrics.snapshot() }
func (r *Relay) logMetrics() {
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()
	for range ticker.C {
		media := r.metrics.snapshot()
		rooms := r.state.RuntimeStats()
		r.logger.Info("rtc metrics", "online_users", rooms.OnlineUsers, "rooms", rooms.Rooms, "p2p_sessions", rooms.P2PSessions, "sfu_sessions", rooms.SFUSessions, "relay_sessions", rooms.RelaySessions, "media_rx_packets", media.RXPackets, "media_tx_packets", media.TXPackets, "media_rx_bytes", media.RXBytes, "media_tx_bytes", media.TXBytes, "media_auth_failures", media.AuthFailures, "media_muted_drops", media.MutedDrops, "media_invalid_packets", media.InvalidPackets, "media_invalid_registrations", media.InvalidRegistrations, "media_invalid_sources", media.InvalidSources, "media_write_failures", media.WriteFailures, "media_worker_queue_depth", media.WorkerQueueDepth, "media_worker_queue_high_water", media.WorkerQueueHighWater, "media_worker_queue_drops", media.WorkerQueueDrops, "media_stale_queue_drops", media.StaleQueueDrops, "media_worker_packets", media.WorkerPackets, "media_queue_wait_max_us", media.QueueWaitMaxMicros, "media_writer_queue_depth", media.WriterQueueDepth, "media_writer_queue_high_water", media.WriterQueueHighWater, "media_writer_backpressure_events", media.WriterBackpressureEvents, "media_writer_queue_drops", media.WriterQueueDrops, "media_writer_stale_drops", media.WriterStaleDrops, "media_writer_wait_max_us", media.WriterWaitMaxMicros, "media_write_batches", media.WriteBatches)
	}
}
