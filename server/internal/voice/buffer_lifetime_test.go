package voice

import (
	"bytes"
	"fmt"
	"io"
	"log/slog"
	"net"
	"testing"
	"time"
)

func TestPooledDatagramsReleaseAfterSendOrExpiry(t *testing.T) {
	conn, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	sink, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer sink.Close()
	relay := New("", nil, slog.New(slog.NewTextHandler(io.Discard, nil)))
	writer := &relayWriter{relay: relay, conn: conn}
	a, b, c := relay.getBuffer(), relay.getBuffer(), relay.getBuffer()
	copy(a, []byte("old"))
	copy(b, []byte("one"))
	copy(c, []byte("two"))
	writer.write([]outboundDatagram{{payload: a[:3], buffer: a, target: sink.LocalAddr().(*net.UDPAddr), expiresAt: time.Now().Add(-time.Second)}, {payload: b[:3], buffer: b, target: sink.LocalAddr().(*net.UDPAddr)}, {payload: c[:3], buffer: c, target: sink.LocalAddr().(*net.UDPAddr)}})
	sink.SetReadDeadline(time.Now().Add(time.Second))
	buf := make([]byte, 20)
	for _, want := range []string{"one", "two"} {
		n, _, err := sink.ReadFromUDP(buf)
		if err != nil || !bytes.Equal(buf[:n], []byte(want)) {
			t.Fatalf("payload %q, err %v", buf[:n], err)
		}
	}
	if relay.Metrics().WriterStaleDrops != 1 {
		t.Fatal("expired packet not counted")
	}
	// Returning one allocation twice would let two owners mutate the same packet.
	x, y, z := relay.getBuffer(), relay.getBuffer(), relay.getBuffer()
	if &x[0] == &y[0] || &y[0] == &z[0] || &x[0] == &z[0] {
		t.Fatal("pool handed out aliased buffers")
	}
	relay.putBuffer(x)
	relay.putBuffer(y)
	relay.putBuffer(z)
}

func TestPooledWriterAtVoiceCadence(t *testing.T) {
	conn, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	sink, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer sink.Close()
	relay := New("", nil, slog.New(slog.NewTextHandler(io.Discard, nil)))
	writer := relay.startWriter(conn)
	const frames = 10
	received := make(chan error, 1)
	go func() {
		buf := make([]byte, 32)
		sink.SetReadDeadline(time.Now().Add(3 * time.Second))
		for frame := 0; frame < frames; frame++ {
			for member := 0; member < 4; member++ {
				n, _, e := sink.ReadFromUDP(buf)
				if e != nil {
					received <- e
					return
				}
				if n != 2 || buf[0] != byte(frame) || buf[1] != byte(member) {
					received <- fmt.Errorf("corrupt/out-of-order packet %v", buf[:n])
					return
				}
			}
		}
		received <- nil
	}()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for frame := 0; frame < frames; frame++ {
		<-ticker.C
		batch := outboundBatch{}
		for member := 0; member < 4; member++ {
			buffer := relay.getBuffer()
			buffer[0], buffer[1] = byte(frame), byte(member)
			batch.datagrams = append(batch.datagrams, outboundDatagram{payload: buffer[:2], buffer: buffer, target: sink.LocalAddr().(*net.UDPAddr), expiresAt: time.Now().Add(40 * time.Millisecond)})
		}
		writer.enqueue(batch)
	}
	writer.stop()
	if err := <-received; err != nil {
		t.Fatal(err)
	}
	m := relay.Metrics()
	if m.TXPackets != 40 || m.WriterQueueDrops != 0 || m.WriterStaleDrops != 0 {
		t.Fatalf("unexpected media counts: %+v", m)
	}
}

// The sequential fallback borrows buffers; write owns their final release.
func TestSequentialExpiryLeavesBufferOwnedByCaller(t *testing.T) {
	relay := New("", nil, slog.New(slog.NewTextHandler(io.Discard, nil)))
	writer := &relayWriter{relay: relay}
	buffer := relay.getBuffer()
	datagrams := []outboundDatagram{{payload: buffer[:1], buffer: buffer, expiresAt: time.Now().Add(-time.Second)}}
	writer.writeSequential(datagrams)
	borrowed := relay.getBuffer()
	if &borrowed[0] == &buffer[0] {
		t.Fatal("sequential fallback returned a buffer still owned by its caller")
	}
	writer.release(datagrams)
	relay.putBuffer(borrowed)
}
