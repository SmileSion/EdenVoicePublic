package voice

import (
	"net"
	"runtime"
	"sync"
	"time"

	"golang.org/x/net/ipv4"
	"golang.org/x/net/ipv6"
)

type outboundDatagram struct {
	payload   []byte
	buffer    []byte // owned pool buffer, returned only after send/drop
	target    *net.UDPAddr
	channelID uint64
	expiresAt time.Time // Audio deadline includes time already spent in the worker queue.
}

type outboundBatch struct {
	datagrams []outboundDatagram
}

type relayWriter struct {
	relay     *Relay
	conn      *net.UDPConn
	queue     chan outboundBatch
	batchSize int
	wg        sync.WaitGroup
	ipv4      *ipv4.PacketConn
	ipv6      *ipv6.PacketConn
	messages4 []ipv4.Message
	messages6 []ipv6.Message
	buffers   [][]byte
}

func (r *Relay) startWriter(conn *net.UDPConn) *relayWriter {
	writer := &relayWriter{
		relay: r, conn: conn, queue: make(chan outboundBatch, r.options.WriterQueueSize), batchSize: r.options.WriteBatchSize,
	}
	if runtime.GOOS == "linux" {
		if local, ok := conn.LocalAddr().(*net.UDPAddr); ok && local.IP.To4() == nil {
			writer.ipv6 = ipv6.NewPacketConn(conn)
		} else {
			writer.ipv4 = ipv4.NewPacketConn(conn)
		}
	}
	writer.wg.Add(1)
	go writer.run()
	return writer
}

func (w *relayWriter) enqueue(batch outboundBatch) {
	started := time.Now()
	depth := w.relay.metrics.writerQueueDepth.Add(1)
	select {
	case w.queue <- batch:
		updateMax(&w.relay.metrics.writerQueueHighWater, uint64(min(depth, int64(cap(w.queue)))))
	default:
		// A slow UDP writer must not stall every media worker. Drop this
		// batch; queued audio is independently expired before transmission.
		w.relay.metrics.writerQueueDepth.Add(-1)
		w.relay.metrics.writerBackpressureEvents.Add(1)
		w.relay.metrics.writerQueueDrops.Add(uint64(len(batch.datagrams)))
		w.release(batch.datagrams)
	}
	updateMax(&w.relay.metrics.writerWaitMaxMicros, uint64(time.Since(started).Microseconds()))
}

func (w *relayWriter) release(datagrams []outboundDatagram) {
	for i := range datagrams {
		if datagrams[i].buffer != nil {
			w.relay.putBuffer(datagrams[i].buffer)
		}
		datagrams[i] = outboundDatagram{}
	}
}

func (w *relayWriter) stop() {
	close(w.queue)
	w.wg.Wait()
}

func (w *relayWriter) run() {
	defer w.wg.Done()
	for {
		batch, ok := <-w.queue
		if !ok {
			return
		}
		w.relay.metrics.writerQueueDepth.Add(-1)
		datagrams := batch.datagrams
		closed := false
		for len(datagrams) < w.batchSize {
			select {
			case next, ok := <-w.queue:
				if !ok {
					closed = true
					break
				}
				w.relay.metrics.writerQueueDepth.Add(-1)
				datagrams = append(datagrams, next.datagrams...)
			default:
				closed = false
				break
			}
			if closed || len(datagrams) >= w.batchSize || len(w.queue) == 0 {
				break
			}
		}
		w.flush(datagrams)
		if closed {
			return
		}
	}
}

func (w *relayWriter) flush(datagrams []outboundDatagram) {
	for len(datagrams) > 0 {
		count := min(len(datagrams), w.batchSize)
		w.write(datagrams[:count])
		datagrams = datagrams[count:]
	}
}

func (w *relayWriter) write(datagrams []outboundDatagram) {
	now := time.Now()
	live := datagrams[:0]
	for _, datagram := range datagrams {
		if !datagram.expiresAt.IsZero() && !now.Before(datagram.expiresAt) {
			w.relay.metrics.writerStaleDrops.Add(1)
			if datagram.buffer != nil {
				w.relay.putBuffer(datagram.buffer)
			}
			continue
		}
		live = append(live, datagram)
	}
	datagrams = live
	defer w.release(datagrams)
	if len(datagrams) == 0 {
		return
	}
	// Bound OS send-buffer stalls as well as application queueing.
	deadline := now.Add(w.relay.options.MaxQueueDelay)
	for _, datagram := range datagrams {
		if !datagram.expiresAt.IsZero() && datagram.expiresAt.Before(deadline) {
			deadline = datagram.expiresAt
		}
	}
	if err := w.conn.SetWriteDeadline(deadline); err != nil {
		w.relay.metrics.writeFailures.Add(uint64(len(datagrams)))
		return
	}
	w.relay.metrics.writeBatches.Add(1)
	if w.ipv4 == nil && w.ipv6 == nil {
		w.writeSequential(datagrams)
		return
	}
	var sent int
	var err error
	if w.ipv4 != nil {
		if cap(w.messages4) < len(datagrams) {
			w.messages4 = make([]ipv4.Message, len(datagrams))
		}
		messages := w.messages4[:len(datagrams)]
		if cap(w.buffers) < len(datagrams) {
			w.buffers = make([][]byte, len(datagrams))
		}
		buffers := w.buffers[:len(datagrams)]
		defer clear(messages)
		defer clear(buffers)
		for index, datagram := range datagrams {
			buffers[index] = datagram.payload
			messages[index] = ipv4.Message{Buffers: buffers[index : index+1], Addr: datagram.target}
		}
		sent, err = w.ipv4.WriteBatch(messages, 0)
	} else {
		if cap(w.messages6) < len(datagrams) {
			w.messages6 = make([]ipv6.Message, len(datagrams))
		}
		messages := w.messages6[:len(datagrams)]
		if cap(w.buffers) < len(datagrams) {
			w.buffers = make([][]byte, len(datagrams))
		}
		buffers := w.buffers[:len(datagrams)]
		defer clear(messages)
		defer clear(buffers)
		for index, datagram := range datagrams {
			buffers[index] = datagram.payload
			messages[index] = ipv6.Message{Buffers: buffers[index : index+1], Addr: datagram.target}
		}
		sent, err = w.ipv6.WriteBatch(messages, 0)
	}
	for _, datagram := range datagrams[:sent] {
		w.relay.metrics.txPackets.Add(1)
		w.relay.metrics.txBytes.Add(uint64(len(datagram.payload)))
		w.relay.metrics.observeRoomTX(datagram.channelID, len(datagram.payload))
	}
	if err == nil && sent == len(datagrams) {
		return
	}
	// Preserve delivery on platforms or kernels where batch writes are not
	// available, and retry only the unsent suffix after a partial send.
	w.writeSequential(datagrams[sent:])
}

func (w *relayWriter) writeSequential(datagrams []outboundDatagram) {
	for _, datagram := range datagrams {
		if !datagram.expiresAt.IsZero() && !time.Now().Before(datagram.expiresAt) {
			w.relay.metrics.writerStaleDrops.Add(1)
			continue
		}
		if _, err := w.conn.WriteToUDP(datagram.payload, datagram.target); err != nil {
			w.relay.metrics.writeFailures.Add(1)
			w.relay.warn(&w.relay.writeFailureLogs, "voice relay write failed", "target", datagram.target, "error", err)
		} else {
			w.relay.metrics.txPackets.Add(1)
			w.relay.metrics.txBytes.Add(uint64(len(datagram.payload)))
			w.relay.metrics.observeRoomTX(datagram.channelID, len(datagram.payload))
		}
	}
}
