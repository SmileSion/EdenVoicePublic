// Command voice-load runs a long-lived encrypted SFU load and impairment test.
package main

import (
	"context"
	cryptorand "crypto/rand"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"math/rand"
	"net"
	"os"
	"os/signal"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
)

type options struct {
	rooms, usersPerRoom, talkersPerRoom int
	duration, reportInterval            time.Duration
	lossPercent, reorderPercent         float64
	maxDelay, burstEvery, burstDuration time.Duration
	blackoutEvery, blackoutDuration     time.Duration
	payloadBytes                        int
}

type loadClient struct {
	sessionID     uint64
	roomID        uint64
	conn          *net.UDPConn
	outbound      *security.MediaCipher
	inbound       *security.MediaCipher
	received      atomic.Uint64
	decryptErrors atomic.Uint64
}

type scheduledPacket struct {
	due  time.Time
	data []byte
}

type result struct {
	DurationSeconds float64               `json:"duration_seconds"`
	Rooms           int                   `json:"rooms"`
	Users           int                   `json:"users"`
	Talkers         int                   `json:"talkers"`
	Sent            uint64                `json:"sent"`
	ImpairmentDrops uint64                `json:"impairment_drops"`
	Received        uint64                `json:"received"`
	DecryptErrors   uint64                `json:"decrypt_errors"`
	Server          voice.MetricsSnapshot `json:"server"`
	Status          string                `json:"status"`
}

func main() {
	var opts options
	flag.IntVar(&opts.rooms, "rooms", 20, "number of active rooms")
	flag.IntVar(&opts.usersPerRoom, "users-per-room", 12, "users in each room")
	flag.IntVar(&opts.talkersPerRoom, "talkers-per-room", 4, "simultaneous talkers in each room")
	flag.DurationVar(&opts.duration, "duration", 2*time.Hour, "test duration (use 8h for soak testing)")
	flag.DurationVar(&opts.reportInterval, "report-interval", 10*time.Second, "progress report interval")
	flag.Float64Var(&opts.lossPercent, "loss-percent", 1, "independent ingress loss percentage")
	flag.Float64Var(&opts.reorderPercent, "reorder-percent", 5, "packets given extra delay to cause reordering")
	flag.DurationVar(&opts.maxDelay, "max-delay", 30*time.Millisecond, "maximum injected ingress delay")
	flag.DurationVar(&opts.burstEvery, "burst-every", 30*time.Second, "interval between burst-loss windows")
	flag.DurationVar(&opts.burstDuration, "burst-duration", 300*time.Millisecond, "burst-loss window duration")
	flag.DurationVar(&opts.blackoutEvery, "blackout-every", 5*time.Minute, "interval between simulated network outages")
	flag.DurationVar(&opts.blackoutDuration, "blackout-duration", 2*time.Second, "simulated network outage duration")
	flag.IntVar(&opts.payloadBytes, "payload-bytes", 80, "synthetic Opus payload size")
	flag.Parse()
	if err := validate(opts); err != nil {
		fmt.Fprintln(os.Stderr, "voice-load:", err)
		os.Exit(2)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx, opts); err != nil {
		fmt.Fprintln(os.Stderr, "voice-load:", err)
		os.Exit(1)
	}
}

func validate(opts options) error {
	if opts.rooms <= 0 || opts.usersPerRoom < 2 || opts.talkersPerRoom <= 0 || opts.talkersPerRoom > opts.usersPerRoom {
		return fmt.Errorf("rooms, users-per-room and talkers-per-room are inconsistent")
	}
	if opts.duration <= 0 || opts.reportInterval <= 0 || opts.payloadBytes <= 0 || opts.payloadBytes > protocol.MaxVoicePacketSize-protocol.VoiceHeaderSize-16 {
		return fmt.Errorf("duration, report interval or payload size is invalid")
	}
	if opts.lossPercent < 0 || opts.lossPercent > 100 || opts.reorderPercent < 0 || opts.reorderPercent > 100 || opts.maxDelay < 0 {
		return fmt.Errorf("network impairment values are invalid")
	}
	return nil
}

func run(parent context.Context, opts options) error {
	cfg := config.Default()
	cfg.Network.P2PEnabled = false
	cfg.Channels = make([]config.Channel, opts.rooms)
	for index := range cfg.Channels {
		cfg.Channels[index] = config.Channel{ID: uint64(index + 1), Name: fmt.Sprintf("load-%02d", index+1), NetworkPolicy: "force_sfu"}
	}
	state := serverstate.New(cfg)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	relay := voice.New("", state, logger, voice.Options{})
	serverConn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		return err
	}
	defer serverConn.Close()
	go func() { _ = relay.Serve(serverConn) }()

	clients, err := createClients(state, serverConn.LocalAddr().(*net.UDPAddr), opts)
	if err != nil {
		return err
	}
	defer closeClients(clients)
	ctx, cancel := context.WithTimeout(parent, opts.duration)
	defer cancel()
	var sent, impairmentDrops atomic.Uint64
	var wg sync.WaitGroup
	for _, client := range clients {
		wg.Add(1)
		go receive(ctx, &wg, client)
	}
	for room := 0; room < opts.rooms; room++ {
		for talker := 0; talker < opts.talkersPerRoom; talker++ {
			client := clients[room*opts.usersPerRoom+talker]
			wg.Add(1)
			go transmit(ctx, &wg, client, serverConn.LocalAddr().(*net.UDPAddr), opts, int64(room*1000+talker+1), &sent, &impairmentDrops)
		}
	}
	started := time.Now()
	ticker := time.NewTicker(opts.reportInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			wg.Wait()
			return printResult(started, opts, clients, relay, sent.Load(), impairmentDrops.Load())
		case <-ticker.C:
			metrics := relay.Metrics()
			fmt.Fprintf(os.Stderr, "elapsed=%s rx=%d tx=%d worker_depth=%d worker_drops=%d stale=%d writer_depth=%d auth_failures=%d\n",
				time.Since(started).Round(time.Second), metrics.RXPackets, metrics.TXPackets, metrics.WorkerQueueDepth,
				metrics.WorkerQueueDrops, metrics.StaleQueueDrops, metrics.WriterQueueDepth, metrics.AuthFailures)
		}
	}
}

func createClients(state *serverstate.State, serverAddress *net.UDPAddr, opts options) ([]*loadClient, error) {
	clients := make([]*loadClient, 0, opts.rooms*opts.usersPerRoom)
	for room := 1; room <= opts.rooms; room++ {
		for user := 0; user < opts.usersPerRoom; user++ {
			var inboundKey, outboundKey [32]byte
			var inboundNonce, outboundNonce [6]byte
			for _, value := range [][]byte{inboundKey[:], outboundKey[:], inboundNonce[:], outboundNonce[:]} {
				if _, err := cryptorand.Read(value); err != nil {
					return nil, err
				}
			}
			serverInbound, _ := security.NewMediaCipher(inboundKey, inboundNonce, security.InitialKeyEpoch)
			clientOutbound, _ := security.NewMediaCipher(inboundKey, inboundNonce, security.InitialKeyEpoch)
			serverOutbound, _ := security.NewMediaCipher(outboundKey, outboundNonce, security.InitialKeyEpoch)
			clientInbound, _ := security.NewMediaCipher(outboundKey, outboundNonce, security.InitialKeyEpoch)
			session := state.AddAuthenticatedSession(fmt.Sprintf("load-%d-%d", room, user), "", "", "", nil, serverInbound, serverOutbound)
			if err := state.Join(session.ID, uint64(room)); err != nil {
				return nil, err
			}
			conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
			if err != nil {
				return nil, err
			}
			if _, err := state.RefreshUDP(session.ID, conn.LocalAddr().(*net.UDPAddr)); err != nil {
				return nil, err
			}
			clients = append(clients, &loadClient{sessionID: session.ID, roomID: uint64(room), conn: conn, outbound: clientOutbound, inbound: clientInbound})
		}
	}
	_ = serverAddress
	return clients, nil
}

func receive(ctx context.Context, wg *sync.WaitGroup, client *loadClient) {
	defer wg.Done()
	buffer := make([]byte, protocol.MaxVoicePacketSize)
	for {
		_ = client.conn.SetReadDeadline(time.Now().Add(250 * time.Millisecond))
		n, _, err := client.conn.ReadFromUDP(buffer)
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			continue
		}
		packet, err := protocol.ParseVoicePacket(buffer[:n])
		if err != nil {
			client.decryptErrors.Add(1)
			continue
		}
		if _, err := client.inbound.Open(buffer[:protocol.VoiceHeaderSize], packet.SessionID, packet.KeyEpoch, packet.Sequence, packet.Payload); err != nil {
			client.decryptErrors.Add(1)
			continue
		}
		client.received.Add(1)
	}
}

func transmit(ctx context.Context, wg *sync.WaitGroup, client *loadClient, server *net.UDPAddr, opts options, seed int64, sent, dropped *atomic.Uint64) {
	defer wg.Done()
	rng := rand.New(rand.NewSource(seed))
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	started := time.Now()
	sequence := uint32(1)
	pending := make([]scheduledPacket, 0, 8)
	payload := make([]byte, opts.payloadBytes)
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-ticker.C:
			for index := 0; index < len(pending); {
				if pending[index].due.After(now) {
					index++
					continue
				}
				_, _ = client.conn.WriteToUDP(pending[index].data, server)
				sent.Add(1)
				pending[index] = pending[len(pending)-1]
				pending = pending[:len(pending)-1]
			}
			elapsed := now.Sub(started)
			blackout := opts.blackoutEvery > 0 && elapsed%opts.blackoutEvery < opts.blackoutDuration
			burst := opts.burstEvery > 0 && elapsed%opts.burstEvery < opts.burstDuration
			if blackout || burst || rng.Float64()*100 < opts.lossPercent {
				dropped.Add(1)
				sequence++
				continue
			}
			_, _ = rng.Read(payload)
			packet := protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: client.sessionID, Sequence: sequence, Timestamp: sequence * 960, KeyEpoch: security.InitialKeyEpoch}
			packet, err := client.outbound.SealPacket(packet, payload)
			if err != nil {
				return
			}
			encoded, _ := packet.MarshalBinary()
			delay := time.Duration(0)
			if opts.maxDelay > 0 {
				delay = time.Duration(rng.Int63n(int64(opts.maxDelay) + 1))
			}
			if rng.Float64()*100 < opts.reorderPercent {
				delay += 40 * time.Millisecond
			}
			pending = append(pending, scheduledPacket{due: now.Add(delay), data: encoded})
			sequence++
		}
	}
}

func closeClients(clients []*loadClient) {
	for _, client := range clients {
		_ = client.conn.Close()
		client.outbound.Destroy()
		client.inbound.Destroy()
	}
}

func printResult(started time.Time, opts options, clients []*loadClient, relay *voice.Relay, sent, impairmentDrops uint64) error {
	var received, decryptErrors uint64
	for _, client := range clients {
		received += client.received.Load()
		decryptErrors += client.decryptErrors.Load()
	}
	server := relay.Metrics()
	status := "ok"
	if decryptErrors > 0 || server.AuthFailures > 0 || server.WriteFailures > 0 {
		status = "failed"
	}
	value := result{DurationSeconds: time.Since(started).Seconds(), Rooms: opts.rooms, Users: len(clients), Talkers: opts.rooms * opts.talkersPerRoom, Sent: sent, ImpairmentDrops: impairmentDrops, Received: received, DecryptErrors: decryptErrors, Server: server, Status: status}
	encoded, _ := json.MarshalIndent(value, "", "  ")
	fmt.Println(string(encoded))
	if status != "ok" {
		return fmt.Errorf("load test detected authentication, decryption or write failures")
	}
	return nil
}
