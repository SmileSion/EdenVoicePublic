package voice

import "testing"

func TestRoomMetricsTrackRatesAndRemainSorted(t *testing.T) {
	var metrics mediaMetrics
	metrics.observeRoomRX(20, 100)
	metrics.observeRoomTX(20, 80)
	metrics.observeRoomRX(10, 120)
	rooms := metrics.snapshot().Rooms
	if len(rooms) != 2 || rooms[0].ChannelID != 10 || rooms[1].ChannelID != 20 {
		t.Fatalf("room metrics are not stable and sorted: %+v", rooms)
	}
	if rooms[1].RXPackets != 1 || rooms[1].TXPackets != 1 || rooms[1].RXBytes != 100 || rooms[1].TXBytes != 80 {
		t.Fatalf("unexpected room counters: %+v", rooms[1])
	}
	if rooms[1].RXPacketsPerSecond <= 0 || rooms[1].TXPacketsPerSecond <= 0 {
		t.Fatalf("room packet rates must be positive: %+v", rooms[1])
	}
}
