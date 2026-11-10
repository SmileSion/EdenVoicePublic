package voice

import (
	"sort"
	"sync"
	"sync/atomic"
	"time"
)

type MetricsSnapshot struct {
	RXPackets, TXPackets, RXBytes, TXBytes, AuthFailures, MutedDrops    uint64
	InvalidPackets, InvalidRegistrations, InvalidSources, WriteFailures uint64
	WorkerQueueDrops, StaleQueueDrops, WorkerPackets                    uint64
	WorkerQueueDepth                                                    int64
	WorkerQueueHighWater, QueueWaitMaxMicros                            uint64
	WriterBackpressureEvents, WriteBatches                              uint64
	WriterQueueDrops, WriterStaleDrops                                  uint64
	WriterQueueDepth                                                    int64
	WriterQueueHighWater, WriterWaitMaxMicros                           uint64
	Rooms                                                               []RoomMetricsSnapshot
}

type RoomMetricsSnapshot struct {
	ChannelID          uint64
	RXPackets          uint64
	TXPackets          uint64
	RXBytes            uint64
	TXBytes            uint64
	RXPacketsPerSecond float64
	TXPacketsPerSecond float64
}

type roomMediaMetrics struct {
	rxPackets uint64
	txPackets uint64
	rxBytes   uint64
	txBytes   uint64
	startedAt time.Time
}

type mediaMetrics struct {
	rxPackets, txPackets, rxBytes, txBytes, authFailures, mutedDrops    atomic.Uint64
	invalidPackets, invalidRegistrations, invalidSources, writeFailures atomic.Uint64
	workerQueueDrops, staleQueueDrops, workerPackets                    atomic.Uint64
	workerQueueDepth                                                    atomic.Int64
	workerQueueHighWater, queueWaitMaxMicros                            atomic.Uint64
	writerBackpressureEvents, writeBatches                              atomic.Uint64
	writerQueueDrops, writerStaleDrops                                  atomic.Uint64
	writerQueueDepth                                                    atomic.Int64
	writerQueueHighWater, writerWaitMaxMicros                           atomic.Uint64
	roomsMu                                                             sync.Mutex
	rooms                                                               map[uint64]*roomMediaMetrics
}

func (m *mediaMetrics) observeRoomRX(channelID uint64, bytes int) {
	if channelID == 0 {
		return
	}
	m.roomsMu.Lock()
	room := m.room(channelID)
	room.rxPackets++
	room.rxBytes += uint64(bytes)
	m.roomsMu.Unlock()
}

func (m *mediaMetrics) observeRoomTX(channelID uint64, bytes int) {
	if channelID == 0 {
		return
	}
	m.roomsMu.Lock()
	room := m.room(channelID)
	room.txPackets++
	room.txBytes += uint64(bytes)
	m.roomsMu.Unlock()
}

func (m *mediaMetrics) room(channelID uint64) *roomMediaMetrics {
	if m.rooms == nil {
		m.rooms = make(map[uint64]*roomMediaMetrics)
	}
	room := m.rooms[channelID]
	if room == nil {
		room = &roomMediaMetrics{startedAt: time.Now()}
		m.rooms[channelID] = room
	}
	return room
}

func (m *mediaMetrics) roomSnapshot() []RoomMetricsSnapshot {
	m.roomsMu.Lock()
	defer m.roomsMu.Unlock()
	now := time.Now()
	rooms := make([]RoomMetricsSnapshot, 0, len(m.rooms))
	for channelID, room := range m.rooms {
		seconds := now.Sub(room.startedAt).Seconds()
		if seconds < 1 {
			seconds = 1
		}
		rooms = append(rooms, RoomMetricsSnapshot{
			ChannelID: channelID, RXPackets: room.rxPackets, TXPackets: room.txPackets,
			RXBytes: room.rxBytes, TXBytes: room.txBytes,
			RXPacketsPerSecond: float64(room.rxPackets) / seconds,
			TXPacketsPerSecond: float64(room.txPackets) / seconds,
		})
	}
	sort.Slice(rooms, func(i, j int) bool { return rooms[i].ChannelID < rooms[j].ChannelID })
	return rooms
}

func (m *mediaMetrics) observeQueueDepth(depth uint64) {
	updateMax(&m.workerQueueHighWater, depth)
}

func (m *mediaMetrics) observeQueueWait(wait time.Duration) {
	updateMax(&m.queueWaitMaxMicros, uint64(wait.Microseconds()))
}

func updateMax(value *atomic.Uint64, candidate uint64) {
	for current := value.Load(); candidate > current; current = value.Load() {
		if value.CompareAndSwap(current, candidate) {
			return
		}
	}
}

func (m *mediaMetrics) snapshot() MetricsSnapshot {
	return MetricsSnapshot{
		RXPackets: m.rxPackets.Load(), TXPackets: m.txPackets.Load(), RXBytes: m.rxBytes.Load(), TXBytes: m.txBytes.Load(),
		AuthFailures: m.authFailures.Load(), MutedDrops: m.mutedDrops.Load(), InvalidPackets: m.invalidPackets.Load(),
		InvalidRegistrations: m.invalidRegistrations.Load(), InvalidSources: m.invalidSources.Load(), WriteFailures: m.writeFailures.Load(),
		WorkerQueueDrops: m.workerQueueDrops.Load(), StaleQueueDrops: m.staleQueueDrops.Load(), WorkerPackets: m.workerPackets.Load(),
		WorkerQueueDepth: m.workerQueueDepth.Load(), WorkerQueueHighWater: m.workerQueueHighWater.Load(), QueueWaitMaxMicros: m.queueWaitMaxMicros.Load(),
		WriterBackpressureEvents: m.writerBackpressureEvents.Load(), WriteBatches: m.writeBatches.Load(), WriterQueueDepth: m.writerQueueDepth.Load(), WriterQueueHighWater: m.writerQueueHighWater.Load(), WriterWaitMaxMicros: m.writerWaitMaxMicros.Load(),
		WriterQueueDrops: m.writerQueueDrops.Load(), WriterStaleDrops: m.writerStaleDrops.Load(),
		Rooms: m.roomSnapshot(),
	}
}
