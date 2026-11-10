package screenshare_test

import (
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
	"encoding/binary"
	"io"
	"log/slog"
	"net"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// Exercises the real UDP voice relay alongside browser video. Synthetic voice
// payloads verify transport continuity, not Opus decoding or subjective sound.
func concurrentVoice(t *testing.T) func() {
	t.Helper()
	state := serverstate.New(config.Default())
	socket, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
	if err != nil {
		t.Fatal(err)
	}
	relay := voice.New("", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	relayDone := make(chan struct{})
	go func() { defer close(relayDone); _ = relay.Serve(socket) }()
	var clients []*net.UDPConn
	var ids []uint64
	for i := 0; i < 5; i++ {
		session := state.AddSession("screen-concurrency", nil)
		if err := state.Join(session.ID, 2); err != nil {
			t.Fatal(err)
		}
		conn, e := net.ListenUDP("udp", &net.UDPAddr{IP: net.ParseIP("127.0.0.1")})
		if e != nil {
			t.Fatal(e)
		}
		clients = append(clients, conn)
		ids = append(ids, session.ID)
		packet, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeRegister, SessionID: session.ID}).MarshalBinary()
		_, _ = conn.WriteToUDP(packet, socket.LocalAddr().(*net.UDPAddr))
	}
	t.Cleanup(func() {
		for _, c := range clients {
			_ = c.Close()
		}
		_ = socket.Close()
		<-relayDone
	})
	for i, c := range clients {
		waitVoice(t, func() bool { _, ok := state.ValidateUDP(ids[i], c.LocalAddr().(*net.UDPAddr)); return ok })
	}
	var sent, received, invalid, maxDelay atomic.Uint64
	var readers sync.WaitGroup
	for i, c := range clients {
		readers.Add(1)
		go func(i int, c *net.UDPConn) {
			defer readers.Done()
			buffer := make([]byte, protocol.MaxVoicePacketSize)
			previous := map[uint64]uint32{}
			for {
				n, _, err := c.ReadFromUDP(buffer)
				if err != nil {
					return
				}
				p, e := protocol.ParseVoicePacket(buffer[:n])
				if e != nil || p.Type != protocol.VoiceTypeAudio || p.SessionID == ids[i] || len(p.Payload) != 120 {
					invalid.Add(1)
					continue
				}
				if p.Sequence != previous[p.SessionID]+1 {
					invalid.Add(1)
				}
				previous[p.SessionID] = p.Sequence
				delay := uint64(max(0, time.Now().UnixNano()-int64(binary.BigEndian.Uint64(p.Payload[:8]))))
				for old := maxDelay.Load(); delay > old; old = maxDelay.Load() {
					if maxDelay.CompareAndSwap(old, delay) {
						break
					}
				}
				received.Add(1)
			}
		}(i, c)
	}
	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		ticker := time.NewTicker(20 * time.Millisecond)
		defer ticker.Stop()
		var seq uint32
		for {
			select {
			case <-stop:
				return
			case <-ticker.C:
				seq++
				for i, c := range clients {
					payload := make([]byte, 120)
					binary.BigEndian.PutUint64(payload, uint64(time.Now().UnixNano()))
					p, _ := (protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: ids[i], Sequence: seq, Payload: payload}).MarshalBinary()
					if _, err := c.WriteToUDP(p, socket.LocalAddr().(*net.UDPAddr)); err != nil {
						invalid.Add(1)
					} else {
						sent.Add(1)
					}
				}
			}
		}
	}()
	return func() {
		close(stop)
		<-done
		deadline := time.Now().Add(2 * time.Second)
		for received.Load() < sent.Load()*4 && time.Now().Before(deadline) {
			time.Sleep(10 * time.Millisecond)
		}
		for _, c := range clients {
			_ = c.Close()
		}
		readers.Wait()
		t.Logf("concurrent voice: sent=%d expected forwarded=%d received=%d max local delay=%s", sent.Load(), sent.Load()*4, received.Load(), time.Duration(maxDelay.Load()))
		if sent.Load() < 50 || received.Load() != sent.Load()*4 || invalid.Load() != 0 {
			t.Errorf("voice transport discontinuity: sent=%d received=%d invalid=%d", sent.Load(), received.Load(), invalid.Load())
		}
	}
}

func waitVoice(t *testing.T, f func() bool) {
	t.Helper()
	until := time.Now().Add(5 * time.Second)
	for time.Now().Before(until) {
		if f() {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("voice registration timed out")
}
