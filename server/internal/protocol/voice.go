package protocol

import (
	"encoding/binary"
	"errors"
	"fmt"
)

const (
	VoiceMagic         uint16 = 0x4544
	VoiceVersion       uint8  = 2
	VoiceHeaderSize           = 26
	MaxVoicePacketSize        = 1500
)

type VoicePacketType uint8

const (
	VoiceTypeAudio    VoicePacketType = 1
	VoiceTypeRegister VoicePacketType = 2
	VoiceTypeProbe    VoicePacketType = 3
	VoiceTypeProbeAck VoicePacketType = 4
)

type VoicePacket struct {
	Type      VoicePacketType
	Flags     uint16
	SessionID uint64
	Sequence  uint32
	Timestamp uint32
	KeyEpoch  uint16
	Payload   []byte
}

func (p VoicePacket) MarshalBinary() ([]byte, error) {
	return p.AppendBinary(nil)
}

// AppendBinary preserves the wire format and can reuse caller-owned capacity.
func (p VoicePacket) AppendBinary(dst []byte) ([]byte, error) {
	if len(p.Payload) > MaxVoicePacketSize-VoiceHeaderSize {
		return nil, errors.New("voice payload too large")
	}
	offset := len(dst)
	size := VoiceHeaderSize + len(p.Payload)
	if cap(dst)-offset >= size {
		dst = dst[:offset+size]
	} else {
		grown := make([]byte, offset+size)
		copy(grown, dst)
		dst = grown
	}
	b := dst[offset:]
	binary.BigEndian.PutUint16(b[0:2], VoiceMagic)
	b[2] = VoiceVersion
	b[3] = byte(p.Type)
	binary.BigEndian.PutUint16(b[4:6], p.Flags)
	binary.BigEndian.PutUint64(b[6:14], p.SessionID)
	binary.BigEndian.PutUint32(b[14:18], p.Sequence)
	binary.BigEndian.PutUint32(b[18:22], p.Timestamp)
	binary.BigEndian.PutUint16(b[22:24], p.KeyEpoch)
	binary.BigEndian.PutUint16(b[24:26], uint16(len(p.Payload)))
	copy(b[VoiceHeaderSize:], p.Payload)
	return dst, nil
}

func (p VoicePacket) AuthenticatedHeader(payloadLength int) ([]byte, error) {
	if payloadLength < 0 || payloadLength > MaxVoicePacketSize-VoiceHeaderSize {
		return nil, errors.New("voice payload too large")
	}
	header := make([]byte, VoiceHeaderSize)
	binary.BigEndian.PutUint16(header[0:2], VoiceMagic)
	header[2] = VoiceVersion
	header[3] = byte(p.Type)
	binary.BigEndian.PutUint16(header[4:6], p.Flags)
	binary.BigEndian.PutUint64(header[6:14], p.SessionID)
	binary.BigEndian.PutUint32(header[14:18], p.Sequence)
	binary.BigEndian.PutUint32(header[18:22], p.Timestamp)
	binary.BigEndian.PutUint16(header[22:24], p.KeyEpoch)
	binary.BigEndian.PutUint16(header[24:26], uint16(payloadLength))
	return header, nil
}

func ParseVoicePacket(b []byte) (VoicePacket, error) {
	if len(b) < VoiceHeaderSize || len(b) > MaxVoicePacketSize {
		return VoicePacket{}, errors.New("invalid voice packet size")
	}
	if binary.BigEndian.Uint16(b[0:2]) != VoiceMagic || b[2] != VoiceVersion {
		return VoicePacket{}, errors.New("invalid voice magic or version")
	}
	payloadLen := int(binary.BigEndian.Uint16(b[24:26]))
	if payloadLen != len(b)-VoiceHeaderSize {
		return VoicePacket{}, fmt.Errorf("payload length mismatch: header=%d actual=%d", payloadLen, len(b)-VoiceHeaderSize)
	}
	typeValue := VoicePacketType(b[3])
	if typeValue != VoiceTypeAudio && typeValue != VoiceTypeRegister && typeValue != VoiceTypeProbe && typeValue != VoiceTypeProbeAck {
		return VoicePacket{}, errors.New("unknown voice packet type")
	}
	return VoicePacket{
		Type:      typeValue,
		Flags:     binary.BigEndian.Uint16(b[4:6]),
		SessionID: binary.BigEndian.Uint64(b[6:14]),
		Sequence:  binary.BigEndian.Uint32(b[14:18]),
		Timestamp: binary.BigEndian.Uint32(b[18:22]),
		KeyEpoch:  binary.BigEndian.Uint16(b[22:24]),
		// Payload is a view into b so relay callers can validate and forward a
		// datagram without allocating. Callers that retain it must clone it.
		Payload: b[VoiceHeaderSize:],
	}, nil
}
