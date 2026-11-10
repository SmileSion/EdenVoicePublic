package voice

import (
	"bytes"
	"fmt"
	"io"
	"log/slog"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
)

type relayRecordingSender struct {
	mu       sync.Mutex
	messages []protocol.Envelope
}

func (s *relayRecordingSender) Send(message protocol.Envelope) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.messages = append(s.messages, message)
	return nil
}

func (s *relayRecordingSender) count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.messages)
}

func BenchmarkSecureSFUDecryptAndFiveWayFanout(b *testing.B) {
	var key [32]byte
	var prefix [6]byte
	inboundSender, _ := security.NewMediaCipher(key, prefix, 1)
	inboundReceiver, _ := security.NewMediaCipher(key, prefix, 1)
	outbound := make([]*security.MediaCipher, 5)
	for index := range outbound {
		outbound[index], _ = security.NewMediaCipher(key, prefix, 1)
	}
	aad := make([]byte, protocol.VoiceHeaderSize)
	payload := make([]byte, 120)
	b.ReportAllocs()
	b.SetBytes(int64(len(payload)))
	for index := 0; index < b.N; index++ {
		sequence := uint32(index)
		ciphertext, epoch, err := inboundSender.Seal(aad, 42, sequence, payload)
		if err != nil {
			b.Fatal(err)
		}
		plaintext, err := inboundReceiver.Open(aad, 42, epoch, sequence, ciphertext)
		if err != nil {
			b.Fatal(err)
		}
		for recipient, cipher := range outbound {
			if _, _, err := cipher.Seal(aad, uint64(recipient+100), sequence, plaintext); err != nil {
				b.Fatal(err)
			}
		}
	}
}

func BenchmarkRelayConcurrentRoomFanout(b *testing.B) {
	for _, workerCount := range []int{1, 2, 4, 8} {
		b.Run(fmt.Sprintf("workers-%d", workerCount), func(b *testing.B) {
			const roomCount = 16
			cfg := config.Default()
			cfg.Channels = make([]config.Channel, roomCount)
			for room := range roomCount {
				cfg.Channels[room] = config.Channel{ID: uint64(room + 1), Name: fmt.Sprintf("room-%d", room)}
			}
			state := serverstate.New(cfg)
			server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
			if err != nil {
				b.Fatal(err)
			}
			sink, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
			if err != nil {
				server.Close()
				b.Fatal(err)
			}
			_ = sink.SetReadBuffer(4 << 20)
			var drainWG sync.WaitGroup
			drainWG.Add(1)
			go func() {
				defer drainWG.Done()
				buffer := make([]byte, protocol.MaxVoicePacketSize)
				for {
					if _, _, err := sink.ReadFromUDP(buffer); err != nil {
						return
					}
				}
			}()

			type sender struct {
				id     uint64
				source *net.UDPAddr
			}
			senders := make([]sender, roomCount)
			for room := range roomCount {
				roomID := uint64(room + 1)
				for participant := range 5 {
					session := state.AddSession(fmt.Sprintf("room-%d-user-%d", room, participant), nil)
					if err := state.Join(session.ID, roomID); err != nil {
						b.Fatal(err)
					}
					address := sink.LocalAddr().(*net.UDPAddr)
					if participant == 0 {
						address = &net.UDPAddr{IP: net.ParseIP("127.0.0.1"), Port: 20_000 + room}
						senders[room] = sender{id: session.ID, source: address}
					}
					if err := state.RegisterUDP(session.ID, address); err != nil {
						b.Fatal(err)
					}
				}
			}
			relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)), Options{
				WorkerCount: workerCount, WorkerQueueSize: 1024, MaxQueueDelay: time.Hour,
			})
			writer := relay.startWriter(server)
			workers := relay.startWorkers(writer)
			payload := make([]byte, 120)
			b.ReportAllocs()
			b.SetBytes(int64(len(payload)))
			b.ResetTimer()
			for index := 0; index < b.N; index++ {
				sender := senders[index%len(senders)]
				encoded, err := (protocol.VoicePacket{
					Type: protocol.VoiceTypeAudio, SessionID: sender.id, Sequence: uint32(index), Payload: payload,
				}).MarshalBinary()
				if err != nil {
					b.Fatal(err)
				}
				buffer := relay.getBuffer()
				copy(buffer, encoded)
				packet, err := protocol.ParseVoicePacket(buffer[:len(encoded)])
				if err != nil {
					b.Fatal(err)
				}
				worker := &workers[relayShard(sender.id, len(workers))]
				depth := relay.metrics.workerQueueDepth.Add(1)
				worker.jobs <- relayJob{packet: packet, source: sender.source, buffer: buffer, length: len(encoded), receivedAt: time.Now()}
				relay.metrics.observeQueueDepth(uint64(depth))
			}
			relay.stopWorkers(workers)
			writer.stop()
			b.StopTimer()
			// This is deliberately unpaced saturation, not a real-time call.
			// Account for bounded-queue drops instead of claiming full delivery.
			metrics := relay.Metrics()
			drops := metrics.WriterQueueDrops + metrics.WriterStaleDrops
			if got, want := metrics.TXPackets+drops, uint64(b.N*4); got != want {
				b.Fatalf("forwarded + explicitly dropped packets = %d, want %d", got, want)
			}
			b.ReportMetric(float64(metrics.TXPackets)/float64(b.N), "forwarded/op")
			b.ReportMetric(100*float64(drops)/float64(b.N*4), "writer-drop%")
			server.Close()
			sink.Close()
			drainWG.Wait()
		})
	}
}

func TestRelayShardIsStableAndDistributesSessions(t *testing.T) {
	const workers = 8
	used := make(map[int]bool, workers)
	for sessionID := uint64(1); sessionID <= 1_000; sessionID++ {
		first := relayShard(sessionID, workers)
		if second := relayShard(sessionID, workers); second != first {
			t.Fatalf("session %d moved from shard %d to %d", sessionID, first, second)
		}
		used[first] = true
	}
	if len(used) != workers {
		t.Fatalf("session hashing used %d/%d workers", len(used), workers)
	}
}

func TestRelayQueueIsBoundedAndCountsDrops(t *testing.T) {
	relay := New("", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)), Options{
		WorkerCount: 1, WorkerQueueSize: 1, MaxQueueDelay: time.Second,
	})
	workers := []relayWorker{{jobs: make(chan relayJob, 1)}}
	first := relayJob{packet: protocol.VoicePacket{SessionID: 1}, buffer: relay.getBuffer()}
	second := relayJob{packet: protocol.VoicePacket{SessionID: 1}, buffer: relay.getBuffer()}
	if !relay.enqueue(workers, first) {
		t.Fatal("first packet was not queued")
	}
	if relay.enqueue(workers, second) {
		t.Fatal("packet was queued beyond the configured capacity")
	}
	queued := <-workers[0].jobs
	relay.metrics.workerQueueDepth.Add(-1)
	relay.putBuffer(queued.buffer)
	metrics := relay.Metrics()
	if metrics.WorkerQueueDrops != 1 || metrics.WorkerQueueDepth != 0 || metrics.WorkerQueueHighWater != 1 {
		t.Fatalf("unexpected queue metrics: %+v", metrics)
	}
}

func TestRelayWorkerDropsStaleMediaBeforeProcessing(t *testing.T) {
	relay := New("", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)), Options{
		WorkerCount: 1, WorkerQueueSize: 4, MaxQueueDelay: time.Millisecond,
	})
	workers := relay.startWorkers(nil)
	job := relayJob{
		packet:     protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: 1},
		buffer:     relay.getBuffer(),
		receivedAt: time.Now().Add(-time.Second),
	}
	if !relay.enqueue(workers, job) {
		t.Fatal("stale packet was not queued for the worker")
	}
	deadline := time.Now().Add(time.Second)
	for relay.Metrics().StaleQueueDrops != 1 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	relay.stopWorkers(workers)
	metrics := relay.Metrics()
	if metrics.StaleQueueDrops != 1 || metrics.WorkerPackets != 0 || metrics.WorkerQueueDepth != 0 {
		t.Fatalf("unexpected stale packet metrics: %+v", metrics)
	}
}

func TestRelayWriterBackpressureDoesNotBlockMediaWorkers(t *testing.T) {
	relay := New("", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	writer := &relayWriter{relay: relay, queue: make(chan outboundBatch, 1)}
	writer.enqueue(outboundBatch{datagrams: []outboundDatagram{{payload: []byte("queued")}}})
	done := make(chan struct{})
	go func() {
		writer.enqueue(outboundBatch{datagrams: []outboundDatagram{{}, {}}})
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(time.Second):
		// Unblock the old implementation before failing the regression.
		<-writer.queue
		relay.metrics.writerQueueDepth.Add(-1)
		<-done
		t.Fatal("full writer queue blocked a media worker")
	}
	batch := <-writer.queue
	relay.metrics.writerQueueDepth.Add(-1)
	if string(batch.datagrams[0].payload) != "queued" {
		t.Fatal("queue order changed")
	}
	metrics := relay.Metrics()
	if metrics.WriterBackpressureEvents != 1 || metrics.WriterQueueDrops != 2 || metrics.WriterQueueDepth != 0 || metrics.WriterQueueHighWater != 1 {
		t.Fatalf("unexpected writer metrics: %+v", metrics)
	}
}

func TestRelayWriterExpiresQueuedAudioAndStillSendsFreshPackets(t *testing.T) {
	relay := New("", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer sender.Close()
	receiver, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer receiver.Close()
	writer := relay.startWriter(sender)
	target := receiver.LocalAddr().(*net.UDPAddr)
	writer.enqueue(outboundBatch{datagrams: []outboundDatagram{
		{payload: []byte("stale"), target: target, expiresAt: time.Now().Add(-time.Second)},
		{payload: []byte("fresh"), target: target, expiresAt: time.Now().Add(time.Second)},
		{payload: []byte("probe"), target: target},
	}})
	writer.stop()
	receiver.SetReadDeadline(time.Now().Add(time.Second))
	buffer := make([]byte, 32)
	for _, want := range []string{"fresh", "probe"} {
		n, _, err := receiver.ReadFromUDP(buffer)
		if err != nil {
			t.Fatal(err)
		}
		if string(buffer[:n]) != want {
			t.Fatalf("received %q, want %q", buffer[:n], want)
		}
	}
	metrics := relay.Metrics()
	if metrics.WriterStaleDrops != 1 || metrics.TXPackets != 2 || metrics.WriterQueueDepth != 0 || metrics.WriteFailures != 0 {
		t.Fatalf("unexpected expiry metrics: %+v", metrics)
	}
}

func TestDiagnosticLimiterBoundsAndReportsSuppressedEvents(t *testing.T) {
	limiter := newDiagnosticLimiter(10*time.Second, 2)
	start := time.Unix(100, 0)
	for index := range 5 {
		allowed, suppressed := limiter.allow(start.Add(time.Duration(index) * time.Second))
		if allowed != (index < 2) || suppressed != 0 {
			t.Fatalf("event %d: allowed=%v suppressed=%d", index, allowed, suppressed)
		}
	}
	allowed, suppressed := limiter.allow(start.Add(10 * time.Second))
	if !allowed || suppressed != 3 {
		t.Fatalf("new window did not report suppressed events: allowed=%v suppressed=%d", allowed, suppressed)
	}
}

func TestRelayCountsAndSamplesMalformedPackets(t *testing.T) {
	state := serverstate.New(config.Default())
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	var logs bytes.Buffer
	relay := New("", state, slog.New(slog.NewTextHandler(&logs, nil)))
	errCh := make(chan error, 1)
	go func() { errCh <- relay.Serve(server) }()
	client, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	for range 8 {
		if _, err := client.WriteToUDP([]byte{1}, server.LocalAddr().(*net.UDPAddr)); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.Now().Add(time.Second)
	for relay.Metrics().InvalidPackets != 8 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if got := relay.Metrics().InvalidPackets; got != 8 {
		t.Fatalf("invalid packet metric = %d, want 8", got)
	}
	if err := server.Close(); err != nil {
		t.Fatal(err)
	}
	<-errCh
	if got := strings.Count(logs.String(), `msg="invalid voice packet"`); got != 3 {
		t.Fatalf("sampled warning count = %d, want 3", got)
	}
}

func TestRelayEnforcesChannelIsolation(t *testing.T) {
	state := serverstate.New(config.Default())
	a := state.AddSession("A", nil)
	b := state.AddSession("B", nil)
	c := state.AddSession("C", nil)
	if err := state.Join(a.ID, 2); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(b.ID, 2); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(c.ID, 1); err != nil {
		t.Fatal(err)
	}
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	go func() { _ = relay.Serve(server) }()
	clients := make([]*net.UDPConn, 3)
	for i := range clients {
		clients[i], err = net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
		if err != nil {
			t.Fatal(err)
		}
		defer clients[i].Close()
	}
	ids := []uint64{a.ID, b.ID, c.ID}
	for i, client := range clients {
		packet, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: ids[i]}).MarshalBinary()
		if _, err := client.WriteToUDP(packet, server.LocalAddr().(*net.UDPAddr)); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.Now().Add(2 * time.Second)
	for i, client := range clients {
		address := client.LocalAddr().(*net.UDPAddr)
		for {
			if _, ok := state.ValidateUDP(ids[i], address); ok {
				break
			}
			if time.Now().After(deadline) {
				t.Fatal("udp registration timed out")
			}
			time.Sleep(time.Millisecond)
		}
	}
	want, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: a.ID, Sequence: 7, Timestamp: 960, Payload: []byte{1, 2, 3}}).MarshalBinary()
	if _, err := clients[0].WriteToUDP(want, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	clients[1].SetReadDeadline(time.Now().Add(time.Second))
	n, _, err := clients[1].ReadFromUDP(buffer)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(buffer[:n], want) {
		t.Fatal("relay modified voice datagram")
	}
	clients[2].SetReadDeadline(time.Now().Add(100 * time.Millisecond))
	if _, _, err := clients[2].ReadFromUDP(buffer); err == nil {
		t.Fatal("voice leaked into a different channel")
	} else if netErr, ok := err.(net.Error); !ok || !netErr.Timeout() {
		t.Fatalf("unexpected read error: %v", err)
	}
}

func TestRelayRejectsSessionSourceMismatch(t *testing.T) {
	state := serverstate.New(config.Default())
	session := state.AddSession("A", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	registered, _ := net.ResolveUDPAddr("udp", "127.0.0.1:5000")
	if err := state.RegisterUDP(session.ID, registered); err != nil {
		t.Fatal(err)
	}
	wrong, _ := net.ResolveUDPAddr("udp", "127.0.0.1:5001")
	if _, ok := state.ValidateUDP(session.ID, wrong); ok {
		t.Fatal("mismatched source was accepted")
	}
}

func TestRelayAcknowledgesProbeOnlyForRegisteredSource(t *testing.T) {
	state := serverstate.New(config.Default())
	session := state.AddSession("A", nil)
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	go func() { _ = relay.Serve(server) }()
	registered, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer registered.Close()
	attacker, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer attacker.Close()
	registration, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}).MarshalBinary()
	if _, err := registered.WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(time.Second)
	for {
		if _, ok := state.ValidateUDP(session.ID, registered.LocalAddr().(*net.UDPAddr)); ok {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("udp registration timed out")
		}
		time.Sleep(time.Millisecond)
	}

	challenge := []byte("12345678")
	probe, _ := (protocol.VoicePacket{
		Type: protocol.VoiceTypeProbe, SessionID: session.ID, Sequence: 7, Timestamp: 9, Payload: challenge,
	}).MarshalBinary()
	if _, err := registered.WriteToUDP(probe, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	registered.SetReadDeadline(time.Now().Add(time.Second))
	n, source, err := registered.ReadFromUDP(buffer)
	if err != nil {
		t.Fatal(err)
	}
	ack, err := protocol.ParseVoicePacket(buffer[:n])
	if err != nil {
		t.Fatal(err)
	}
	if !source.IP.Equal(server.LocalAddr().(*net.UDPAddr).IP) || ack.Type != protocol.VoiceTypeProbeAck || ack.SessionID != session.ID || ack.Sequence != 7 || ack.Timestamp != 9 || !bytes.Equal(ack.Payload, challenge) {
		t.Fatalf("unexpected probe ack from %s: %+v", source, ack)
	}

	if _, err := attacker.WriteToUDP(probe, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	attacker.SetReadDeadline(time.Now().Add(100 * time.Millisecond))
	if _, _, err := attacker.ReadFromUDP(buffer); err == nil {
		t.Fatal("unregistered source received a probe acknowledgement")
	} else if netErr, ok := err.(net.Error); !ok || !netErr.Timeout() {
		t.Fatalf("unexpected attacker read error: %v", err)
	}
}

func TestRelayAuthenticatesAndEncryptsSecureProbe(t *testing.T) {
	state := serverstate.New(config.Default())
	clientToServerKey := [32]byte{1, 2, 3}
	serverToClientKey := [32]byte{4, 5, 6}
	clientPrefix := [6]byte{7, 8, 9}
	serverPrefix := [6]byte{10, 11, 12}
	clientSender, _ := security.NewMediaCipher(clientToServerKey, clientPrefix, 1)
	serverInbound, _ := security.NewMediaCipher(clientToServerKey, clientPrefix, 1)
	serverOutbound, _ := security.NewMediaCipher(serverToClientKey, serverPrefix, 1)
	clientReceiver, _ := security.NewMediaCipher(serverToClientKey, serverPrefix, 1)
	session := state.AddAuthenticatedSession("A", "client-a", "", "", nil, serverInbound, serverOutbound)

	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	go func() { _ = relay.Serve(server) }()
	client, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()

	encodeSecure := func(packet protocol.VoicePacket, plaintext []byte) []byte {
		packet.KeyEpoch = 1
		header, err := packet.AuthenticatedHeader(len(plaintext) + 16)
		if err != nil {
			t.Fatal(err)
		}
		ciphertext, epoch, err := clientSender.Seal(header, packet.SessionID, packet.Sequence, plaintext)
		if err != nil {
			t.Fatal(err)
		}
		packet.KeyEpoch = epoch
		packet.Payload = ciphertext
		encoded, err := packet.MarshalBinary()
		if err != nil {
			t.Fatal(err)
		}
		return encoded
	}

	registration := encodeSecure(protocol.VoicePacket{
		Type: protocol.VoiceTypeRegister, SessionID: session.ID, Sequence: 0,
	}, nil)
	if _, err := client.WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(time.Second)
	for {
		if _, ok := state.ValidateUDP(session.ID, client.LocalAddr().(*net.UDPAddr)); ok {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("secure udp registration timed out")
		}
		time.Sleep(time.Millisecond)
	}

	challenge := []byte("secure42")
	probe := encodeSecure(protocol.VoicePacket{
		Type: protocol.VoiceTypeProbe, SessionID: session.ID, Sequence: 1,
	}, challenge)
	if _, err := client.WriteToUDP(probe, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	client.SetReadDeadline(time.Now().Add(time.Second))
	n, _, err := client.ReadFromUDP(buffer)
	if err != nil {
		t.Fatal(err)
	}
	ack, err := protocol.ParseVoicePacket(buffer[:n])
	if err != nil {
		t.Fatal(err)
	}
	if ack.Type != protocol.VoiceTypeProbeAck || bytes.Equal(ack.Payload, challenge) {
		t.Fatalf("secure probe ack was not encrypted: %+v", ack)
	}
	plaintext, err := clientReceiver.Open(buffer[:protocol.VoiceHeaderSize], ack.SessionID, ack.KeyEpoch, ack.Sequence, ack.Payload)
	if err != nil || !bytes.Equal(plaintext, challenge) {
		t.Fatalf("secure probe ack failed authentication: plaintext=%x err=%v", plaintext, err)
	}
}

func TestSecureRegistrationMigratesEndpointWithoutReplayingOldBindings(t *testing.T) {
	state := serverstate.New(config.Default())
	key, prefix := [32]byte{42}, [6]byte{7}
	sender, _ := security.NewMediaCipher(key, prefix, 1)
	inbound, _ := security.NewMediaCipher(key, prefix, 1)
	outbound, _ := security.NewMediaCipher([32]byte{43}, [6]byte{8}, 1)
	session := state.AddAuthenticatedSession("listener", "client", "", "", nil, inbound, outbound)
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	oldAddress := &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 4000}
	newAddress := &net.UDPAddr{IP: net.ParseIP("198.51.100.20"), Port: 5000}
	register := func(sequence uint32) []byte {
		packet := protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID, Sequence: sequence, KeyEpoch: 1}
		header, err := packet.AuthenticatedHeader(16)
		if err != nil {
			t.Fatal(err)
		}
		packet.Payload, packet.KeyEpoch, err = sender.Seal(header, session.ID, sequence, nil)
		if err != nil {
			t.Fatal(err)
		}
		data, err := packet.MarshalBinary()
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	apply := func(data []byte, address *net.UDPAddr) {
		packet, err := protocol.ParseVoicePacket(data)
		if err != nil {
			t.Fatal(err)
		}
		relay.handleRegistration(relayJob{packet: packet, source: address, buffer: data, length: len(data)})
	}
	oldRegistration := register(0)
	apply(oldRegistration, oldAddress)
	apply(register(1), newAddress)
	if _, ok := state.ValidateUDP(session.ID, newAddress); !ok {
		t.Fatal("authenticated migration did not replace the endpoint")
	}
	before, _ := state.CandidateSnapshot(session.ID)
	apply(register(2), newAddress)
	after, _ := state.CandidateSnapshot(session.ID)
	if before.Generation != after.Generation {
		t.Fatal("unchanged heartbeat churned candidate generations")
	}
	apply(oldRegistration, oldAddress)
	forged := register(3)
	forged[len(forged)-1] ^= 1
	apply(forged, oldAddress)
	if _, ok := state.ValidateUDP(session.ID, newAddress); !ok {
		t.Fatal("replayed or forged registration replaced the current endpoint")
	}
	if _, ok := state.ValidateUDP(session.ID, oldAddress); ok {
		t.Fatal("old endpoint still accepted")
	}
}

func TestRegistrationChurnUpdatesBindingWithoutBroadcastStorm(t *testing.T) {
	state := serverstate.New(config.Default())
	sender := &relayRecordingSender{}
	session := state.AddSession("speaker", nil)
	listener := state.AddSession("listener", sender)
	for _, id := range []uint64{session.ID, listener.ID} {
		if err := state.Join(id, 1); err != nil {
			t.Fatal(err)
		}
	}
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	register := func(address *net.UDPAddr) {
		packet := protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}
		relay.handleRegistration(relayJob{packet: packet, source: address})
	}
	first := &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 4000}
	latest := &net.UDPAddr{IP: net.ParseIP("192.0.2.10"), Port: 4999}
	register(first)
	announced := sender.count()
	if announced == 0 {
		t.Fatal("initial registration was not announced")
	}
	for port := 4001; port <= latest.Port; port++ {
		register(&net.UDPAddr{IP: latest.IP, Port: port})
	}
	if got := sender.count(); got != announced {
		t.Fatalf("rapid endpoint churn broadcast %d extra control messages", got-announced)
	}
	if _, ok := state.ValidateUDP(session.ID, latest); !ok {
		t.Fatal("latest authenticated endpoint was not installed")
	}
}

func TestLargeSFURoomRegistrationDoesNotFanOutCandidates(t *testing.T) {
	state := serverstate.New(config.Default())
	senders := make([]*relayRecordingSender, 0, 12)
	sessions := make([]*serverstate.Session, 0, 12)
	for index := 0; index < 12; index++ {
		sender := &relayRecordingSender{}
		session := state.AddSession(fmt.Sprintf("member-%d", index), sender)
		if err := state.Join(session.ID, 1); err != nil {
			t.Fatal(err)
		}
		senders = append(senders, sender)
		sessions = append(sessions, session)
	}
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	for index, session := range sessions {
		address := &net.UDPAddr{IP: net.ParseIP("192.0.2.20"), Port: 5000 + index}
		packet := protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}
		relay.handleRegistration(relayJob{packet: packet, source: address})
		if _, ok := state.ValidateUDP(session.ID, address); !ok {
			t.Fatalf("member %d media binding was not installed", index)
		}
	}
	for index, sender := range senders {
		if got := sender.count(); got != 0 {
			t.Fatalf("large-room member %d received %d candidate messages", index, got)
		}
	}
}

func TestRelaySilentlyDropsPacketsQueuedAfterSessionRemoval(t *testing.T) {
	state := serverstate.New(config.Default())
	session := state.AddSession("A", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	var logs bytes.Buffer
	relay := New("", state, slog.New(slog.NewTextHandler(&logs, nil)))
	errCh := make(chan error, 1)
	go func() { errCh <- relay.Serve(server) }()
	client, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	registration, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}).MarshalBinary()
	if _, err := client.WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(time.Second)
	for {
		if _, ok := state.ValidateUDP(session.ID, client.LocalAddr().(*net.UDPAddr)); ok {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("udp registration timed out")
		}
		time.Sleep(time.Millisecond)
	}
	if _, ok := state.RemoveSession(session.ID); !ok {
		t.Fatal("session removal failed")
	}
	latePacket, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: session.ID, Sequence: 1, Payload: []byte{1}}).MarshalBinary()
	if _, err := client.WriteToUDP(latePacket, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	time.Sleep(20 * time.Millisecond)
	if got := relay.Metrics().InvalidSources; got != 0 {
		t.Fatalf("late packet incremented invalid-source metric: %d", got)
	}
	if err := server.Close(); err != nil {
		t.Fatal(err)
	}
	<-errCh
	if strings.Contains(logs.String(), "invalid voice source") {
		t.Fatalf("late packet emitted a security warning: %s", logs.String())
	}
}

func TestRelayStillCountsAnActiveSessionSourceMismatch(t *testing.T) {
	state := serverstate.New(config.Default())
	session := state.AddSession("A", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	var logs bytes.Buffer
	relay := New("", state, slog.New(slog.NewTextHandler(&logs, nil)))
	errCh := make(chan error, 1)
	go func() { errCh <- relay.Serve(server) }()
	registered, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer registered.Close()
	attacker, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer attacker.Close()
	registration, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}).MarshalBinary()
	if _, err := registered.WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(time.Second)
	for {
		if _, ok := state.ValidateUDP(session.ID, registered.LocalAddr().(*net.UDPAddr)); ok {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("udp registration timed out")
		}
		time.Sleep(time.Millisecond)
	}
	packet, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: session.ID, Sequence: 1, Payload: []byte{1}}).MarshalBinary()
	if _, err := attacker.WriteToUDP(packet, server.LocalAddr().(*net.UDPAddr)); err != nil {
		t.Fatal(err)
	}
	deadline = time.Now().Add(time.Second)
	for relay.Metrics().InvalidSources != 1 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if got := relay.Metrics().InvalidSources; got != 1 {
		t.Fatalf("active source mismatch metric = %d, want 1", got)
	}
	if err := server.Close(); err != nil {
		t.Fatal(err)
	}
	<-errCh
	if !strings.Contains(logs.String(), "reason=source_mismatch") {
		t.Fatalf("active source mismatch warning omitted its reason: %s", logs.String())
	}
}

func TestUDPNetworkPreservesConfiguredAddressFamily(t *testing.T) {
	tests := map[string]string{
		"0.0.0.0:9001":   "udp4",
		"127.0.0.1:9001": "udp4",
		"[::]:9001":      "udp6",
		"localhost:9001": "udp",
	}
	for address, expected := range tests {
		actual, err := udpNetwork(address)
		if err != nil {
			t.Fatalf("%s: %v", address, err)
		}
		if actual != expected {
			t.Fatalf("%s: got %s, want %s", address, actual, expected)
		}
	}
}

func TestRelayFiveParticipantsReceiveEveryOtherSpeaker(t *testing.T) {
	state := serverstate.New(config.Default())
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	go func() { _ = relay.Serve(server) }()
	const participantCount = 5
	sessions := make([]*serverstate.Session, participantCount)
	clients := make([]*net.UDPConn, participantCount)
	for i := range participantCount {
		sessions[i] = state.AddSession(string(rune('A'+i)), nil)
		if err := state.Join(sessions[i].ID, 2); err != nil {
			t.Fatal(err)
		}
		clients[i], err = net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
		if err != nil {
			t.Fatal(err)
		}
		defer clients[i].Close()
		registration, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: sessions[i].ID}).MarshalBinary()
		if _, err := clients[i].WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.Now().Add(2 * time.Second)
	for i := range participantCount {
		for {
			if _, ok := state.ValidateUDP(sessions[i].ID, clients[i].LocalAddr().(*net.UDPAddr)); ok {
				break
			}
			if time.Now().After(deadline) {
				t.Fatal("registration timed out")
			}
			time.Sleep(time.Millisecond)
		}
	}
	for i := range participantCount {
		packet, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: sessions[i].ID, Sequence: uint32(i), Payload: []byte{byte(i)}}).MarshalBinary()
		if _, err := clients[i].WriteToUDP(packet, server.LocalAddr().(*net.UDPAddr)); err != nil {
			t.Fatal(err)
		}
	}
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	for receiver := range participantCount {
		clients[receiver].SetReadDeadline(time.Now().Add(2 * time.Second))
		speakers := make(map[uint64]bool)
		for range participantCount - 1 {
			n, _, err := clients[receiver].ReadFromUDP(buffer)
			if err != nil {
				t.Fatalf("receiver %d: %v", receiver, err)
			}
			packet, err := protocol.ParseVoicePacket(buffer[:n])
			if err != nil {
				t.Fatal(err)
			}
			speakers[packet.SessionID] = true
		}
		if len(speakers) != participantCount-1 || speakers[sessions[receiver].ID] {
			t.Fatalf("receiver %d got wrong speakers: %v", receiver, speakers)
		}
	}
}

func TestRelayConcurrentRoomsPreserveOrderAndIsolation(t *testing.T) {
	const (
		roomCount      = 6
		participants   = 3
		packetsPerRoom = 40
	)
	cfg := config.Default()
	cfg.Channels = make([]config.Channel, roomCount)
	for room := range roomCount {
		cfg.Channels[room] = config.Channel{ID: uint64(room + 1), Name: fmt.Sprintf("room-%d", room+1)}
	}
	state := serverstate.New(cfg)
	server, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	relay := New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)), Options{
		WorkerCount: 4, WorkerQueueSize: 256, MaxQueueDelay: time.Second,
	})
	errCh := make(chan error, 1)
	go func() { errCh <- relay.Serve(server) }()

	type roomClients struct {
		sessions [participants]*serverstate.Session
		clients  [participants]*net.UDPConn
	}
	rooms := make([]roomClients, roomCount)
	for room := range roomCount {
		for participant := range participants {
			session := state.AddSession(fmt.Sprintf("room-%d-user-%d", room, participant), nil)
			if err := state.Join(session.ID, uint64(room+1)); err != nil {
				t.Fatal(err)
			}
			client, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
			if err != nil {
				t.Fatal(err)
			}
			rooms[room].sessions[participant] = session
			rooms[room].clients[participant] = client
			defer client.Close()
			registration, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}).MarshalBinary()
			if _, err := client.WriteToUDP(registration, server.LocalAddr().(*net.UDPAddr)); err != nil {
				t.Fatal(err)
			}
		}
	}
	deadline := time.Now().Add(2 * time.Second)
	for room := range roomCount {
		for participant := range participants {
			for {
				if _, ok := state.ValidateUDP(rooms[room].sessions[participant].ID, rooms[room].clients[participant].LocalAddr().(*net.UDPAddr)); ok {
					break
				}
				if time.Now().After(deadline) {
					t.Fatal("concurrent room registration timed out")
				}
				time.Sleep(time.Millisecond)
			}
		}
	}

	var senders sync.WaitGroup
	for room := range roomCount {
		senders.Add(1)
		go func(room int) {
			defer senders.Done()
			for sequence := 1; sequence <= packetsPerRoom; sequence++ {
				packet, _ := (protocol.VoicePacket{
					Type: protocol.VoiceTypeAudio, SessionID: rooms[room].sessions[0].ID,
					Sequence: uint32(sequence), Timestamp: uint32(sequence * 960), Payload: []byte{byte(room)},
				}).MarshalBinary()
				if _, err := rooms[room].clients[0].WriteToUDP(packet, server.LocalAddr().(*net.UDPAddr)); err != nil {
					t.Errorf("room %d send: %v", room, err)
					return
				}
			}
		}(room)
	}
	senders.Wait()

	for room := range roomCount {
		for participant := 1; participant < participants; participant++ {
			client := rooms[room].clients[participant]
			if err := client.SetReadDeadline(time.Now().Add(3 * time.Second)); err != nil {
				t.Fatal(err)
			}
			buffer := make([]byte, protocol.MaxVoicePacketSize)
			for expected := 1; expected <= packetsPerRoom; expected++ {
				n, _, err := client.ReadFromUDP(buffer)
				if err != nil {
					t.Fatalf("room %d participant %d packet %d: %v", room, participant, expected, err)
				}
				packet, err := protocol.ParseVoicePacket(buffer[:n])
				if err != nil {
					t.Fatal(err)
				}
				if packet.SessionID != rooms[room].sessions[0].ID || packet.Sequence != uint32(expected) || !bytes.Equal(packet.Payload, []byte{byte(room)}) {
					t.Fatalf("room %d participant %d received wrong packet: sender=%d sequence=%d payload=%v", room, participant, packet.SessionID, packet.Sequence, packet.Payload)
				}
			}
		}
	}
	if metrics := relay.Metrics(); metrics.WorkerQueueDrops != 0 || metrics.StaleQueueDrops != 0 || metrics.TXPackets != roomCount*(participants-1)*packetsPerRoom || metrics.WriteBatches == 0 {
		t.Fatalf("concurrent rooms dropped media: %+v", metrics)
	}
	if err := server.Close(); err != nil {
		t.Fatal(err)
	}
	<-errCh
}
