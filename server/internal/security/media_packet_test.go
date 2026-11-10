package security

import (
	"bytes"
	"testing"

	"edenvoice/server/internal/protocol"
)

func TestSealPacketUsesCurrentEpochForHeaderAndCiphertext(t *testing.T) {
	for _, kind := range []protocol.VoicePacketType{protocol.VoiceTypeAudio, protocol.VoiceTypeProbeAck, protocol.VoiceTypeRegister} {
		sender, _ := NewMediaCipher([32]byte{}, [6]byte{}, 1)
		receiver, _ := NewMediaCipher([32]byte{}, [6]byte{}, 1)
		packet := protocol.VoicePacket{Type: kind, SessionID: 7, Sequence: 9, Timestamp: 960, KeyEpoch: sender.Epoch()}
		// Deterministically place rekey after the caller observed the old epoch.
		for _, cipher := range []*MediaCipher{sender, receiver} {
			if err := cipher.PrepareRekey([]byte("packet rekey"), 2); err != nil {
				t.Fatal(err)
			}
			if err := cipher.ActivatePrepared(2); err != nil {
				t.Fatal(err)
			}
		}
		plaintext := []byte("voice payload")
		sealed, err := sender.SealPacket(packet, plaintext)
		if err != nil {
			t.Fatal(err)
		}
		if sealed.KeyEpoch != 2 {
			t.Fatalf("epoch = %d", sealed.KeyEpoch)
		}
		wire, err := sealed.MarshalBinary()
		if err != nil {
			t.Fatal(err)
		}
		parsed, err := protocol.ParseVoicePacket(wire)
		if err != nil {
			t.Fatal(err)
		}
		header := wire[:protocol.VoiceHeaderSize]
		tampered := bytes.Clone(header)
		tampered[18] ^= 1
		if _, err := receiver.Open(tampered, parsed.SessionID, parsed.KeyEpoch, parsed.Sequence, parsed.Payload); err == nil {
			t.Fatal("tampered header authenticated")
		}
		got, err := receiver.Open(header, parsed.SessionID, parsed.KeyEpoch, parsed.Sequence, parsed.Payload)
		if err != nil || !bytes.Equal(got, plaintext) {
			t.Fatalf("open: payload=%q err=%v", got, err)
		}
		if _, err := receiver.Open(header, parsed.SessionID, parsed.KeyEpoch, parsed.Sequence, parsed.Payload); err == nil {
			t.Fatal("replayed packet authenticated")
		}
		if packet.KeyEpoch != 1 || packet.Payload != nil {
			t.Fatal("modified caller packet")
		}
	}
}

func TestSealPacketConcurrentRekeyKeepsEveryPacketAuthentic(t *testing.T) {
	for iteration := 0; iteration < 100; iteration++ {
		sender, _ := NewMediaCipher([32]byte{}, [6]byte{}, 1)
		receiver, _ := NewMediaCipher([32]byte{}, [6]byte{}, 1)
		for _, cipher := range []*MediaCipher{sender, receiver} {
			if err := cipher.PrepareRekey([]byte("concurrent rekey"), 2); err != nil {
				t.Fatal(err)
			}
		}
		// Receiver accepts old/current and prepared epochs, as in real handshakes.
		start := make(chan struct{})
		done := make(chan error, 1)
		go func() { <-start; done <- sender.ActivatePrepared(2) }()
		close(start)
		for sequence := uint32(1); sequence <= 20; sequence++ {
			packet := protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: 7, Sequence: sequence, Timestamp: sequence * 960, KeyEpoch: 1}
			sealed, err := sender.SealPacket(packet, []byte{42})
			if err != nil {
				t.Fatal(err)
			}
			wire, err := sealed.MarshalBinary()
			if err != nil {
				t.Fatal(err)
			}
			got, err := receiver.Open(wire[:protocol.VoiceHeaderSize], sealed.SessionID, sealed.KeyEpoch, sealed.Sequence, sealed.Payload)
			if err != nil || !bytes.Equal(got, []byte{42}) {
				t.Fatalf("iteration=%d sequence=%d: %v", iteration, sequence, err)
			}
		}
		if err := <-done; err != nil {
			t.Fatal(err)
		}
	}
}

func TestSealPacketRejectsOversizePayloadAndDestroyedCipher(t *testing.T) {
	cipher, _ := NewMediaCipher([32]byte{}, [6]byte{}, 1)
	packet := protocol.VoicePacket{Type: protocol.VoiceTypeAudio, SessionID: 7, Sequence: 1}
	if _, err := cipher.SealPacket(packet, make([]byte, protocol.MaxVoicePacketSize-protocol.VoiceHeaderSize-16+1)); err == nil {
		t.Fatal("accepted oversized encrypted packet")
	}
	cipher.Destroy()
	if _, err := cipher.SealPacket(packet, nil); err == nil {
		t.Fatal("accepted destroyed cipher")
	}
}
