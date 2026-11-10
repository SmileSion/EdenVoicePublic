package protocol

import (
	"encoding/binary"
	"encoding/hex"
	"testing"
)

func TestVoicePacketGoldenWireFormat(t *testing.T) {
	packet := VoicePacket{Type: VoiceTypeAudio, Flags: 0xa1b2, SessionID: 0x0102030405060708, Sequence: 0x11223344, Timestamp: 0x55667788, Payload: []byte{0xde, 0xad}}
	encoded, err := packet.MarshalBinary()
	if err != nil {
		t.Fatal(err)
	}
	const golden = "45440201a1b20102030405060708112233445566778800000002dead"
	if hex.EncodeToString(encoded) != golden {
		t.Fatalf("wire format mismatch: %x", encoded)
	}
}

func TestVoicePacketRoundTrip(t *testing.T) {
	want := VoicePacket{Type: VoiceTypeAudio, Flags: 3, SessionID: 42, Sequence: ^uint32(0), Timestamp: 960, Payload: []byte{1, 2, 3}}
	encoded, err := want.MarshalBinary()
	if err != nil {
		t.Fatal(err)
	}
	got, err := ParseVoicePacket(encoded)
	if err != nil {
		t.Fatal(err)
	}
	if got.Type != want.Type || got.Flags != want.Flags || got.SessionID != want.SessionID || got.Sequence != want.Sequence || got.Timestamp != want.Timestamp || got.KeyEpoch != want.KeyEpoch || string(got.Payload) != string(want.Payload) {
		t.Fatalf("round trip mismatch: got %+v want %+v", got, want)
	}
}

func TestVoicePacketRejectsInvalidInput(t *testing.T) {
	valid, _ := (VoicePacket{Type: VoiceTypeAudio, SessionID: 1}).MarshalBinary()
	cases := [][]byte{nil, valid[:10], append(append([]byte(nil), valid...), 0)}
	badMagic := append([]byte(nil), valid...)
	binary.BigEndian.PutUint16(badMagic[:2], 0)
	cases = append(cases, badMagic)
	for i, input := range cases {
		if _, err := ParseVoicePacket(input); err == nil {
			t.Errorf("case %d accepted invalid packet", i)
		}
	}
}

func TestAppendVoicePacketReusesCapacityAndPreservesPrefix(t *testing.T) {
	packet := VoicePacket{Type: VoiceTypeAudio, SessionID: 42, Payload: []byte{1, 2, 3}}
	expected, _ := packet.MarshalBinary()
	buffer := make([]byte, 3, 1500)
	copy(buffer, []byte{9, 8, 7})
	ptr := &buffer[0]
	actual, err := packet.AppendBinary(buffer)
	if err != nil {
		t.Fatal(err)
	}
	if &actual[0] != ptr || hex.EncodeToString(actual[3:]) != hex.EncodeToString(expected) || actual[0] != 9 {
		t.Fatal("changed packet or allocation")
	}
	if n := testing.AllocsPerRun(100, func() { _, _ = packet.AppendBinary(buffer) }); n != 0 {
		t.Fatalf("unexpected allocations %v", n)
	}
}

func TestAppendVoicePacketGrowsCapacityAndPreservesPrefix(t *testing.T) {
	packet := VoicePacket{Type: VoiceTypeAudio, SessionID: 42, Payload: []byte{1, 2, 3}}
	prefix := []byte{9, 8, 7}
	buffer := make([]byte, len(prefix), len(prefix))
	copy(buffer, prefix)
	actual, err := packet.AppendBinary(buffer)
	if err != nil {
		t.Fatal(err)
	}
	if hex.EncodeToString(actual[:len(prefix)]) != hex.EncodeToString(prefix) {
		t.Fatal("growth changed prefix")
	}
	decoded, err := ParseVoicePacket(actual[len(prefix):])
	if err != nil || decoded.SessionID != packet.SessionID || hex.EncodeToString(decoded.Payload) != hex.EncodeToString(packet.Payload) {
		t.Fatalf("invalid packet after growth: %+v, %v", decoded, err)
	}
}

var packetBenchmarkSink []byte

func BenchmarkVoiceMarshalBuffer(b *testing.B) {
	packet := VoicePacket{Type: VoiceTypeAudio, Payload: make([]byte, 240)}
	b.Run("allocate", func(b *testing.B) {
		b.ReportAllocs()
		for i := 0; i < b.N; i++ {
			packetBenchmarkSink, _ = packet.MarshalBinary()
		}
	})
	b.Run("reuse", func(b *testing.B) {
		buffer := make([]byte, 1501)
		b.ReportAllocs()
		b.ResetTimer()
		for i := 0; i < b.N; i++ {
			packetBenchmarkSink, _ = packet.AppendBinary(buffer[:0])
		}
	})
}
