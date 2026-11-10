package voice

import (
	"net"
	"sync"
	"time"

	"edenvoice/server/internal/protocol"
)

type relayJob struct {
	packet     protocol.VoicePacket
	source     *net.UDPAddr
	buffer     []byte
	length     int
	receivedAt time.Time
}

type relayWorker struct {
	jobs chan relayJob
	wg   sync.WaitGroup
}

func (r *Relay) startWorkers(writer *relayWriter) []relayWorker {
	workers := make([]relayWorker, r.options.WorkerCount)
	for index := range workers {
		workers[index].jobs = make(chan relayJob, r.options.WorkerQueueSize)
		workers[index].wg.Add(1)
		go func(worker *relayWorker) {
			defer worker.wg.Done()
			for job := range worker.jobs {
				r.metrics.workerQueueDepth.Add(-1)
				waited := time.Since(job.receivedAt)
				r.metrics.observeQueueWait(waited)
				if job.packet.Type == protocol.VoiceTypeAudio && waited > r.options.MaxQueueDelay {
					r.metrics.staleQueueDrops.Add(1)
					r.putBuffer(job.buffer)
					continue
				}
				r.handlePacket(writer, job)
				r.metrics.workerPackets.Add(1)
				r.putBuffer(job.buffer)
			}
		}(&workers[index])
	}
	return workers
}

func (r *Relay) stopWorkers(workers []relayWorker) {
	for index := range workers {
		close(workers[index].jobs)
	}
	for index := range workers {
		workers[index].wg.Wait()
	}
}

func (r *Relay) enqueue(workers []relayWorker, job relayJob) bool {
	worker := &workers[relayShard(job.packet.SessionID, len(workers))]
	depth := r.metrics.workerQueueDepth.Add(1)
	select {
	case worker.jobs <- job:
		r.metrics.observeQueueDepth(uint64(depth))
		return true
	default:
		r.metrics.workerQueueDepth.Add(-1)
		r.metrics.workerQueueDrops.Add(1)
		r.putBuffer(job.buffer)
		return false
	}
}

func relayShard(sessionID uint64, workers int) int {
	// Mix sequential and externally visible session IDs before modulo so the
	// distribution stays stable without relying on the ID allocator's shape.
	value := sessionID
	value ^= value >> 33
	value *= 0xff51afd7ed558ccd
	value ^= value >> 33
	value *= 0xc4ceb9fe1a85ec53
	value ^= value >> 33
	return int(value % uint64(workers))
}

func (r *Relay) getBuffer() []byte {
	return r.bufferPool.Get().([]byte)[:protocol.MaxVoicePacketSize+1]
}

func (r *Relay) putBuffer(buffer []byte) {
	if cap(buffer) >= protocol.MaxVoicePacketSize+1 {
		r.bufferPool.Put(buffer[:protocol.MaxVoicePacketSize+1])
	}
}
