use std::fmt;

pub const MAGIC: u16 = 0x4544;
pub const VERSION: u8 = 2;
pub const HEADER_SIZE: usize = 26;
pub const MAX_PACKET_SIZE: usize = 1500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PacketType {
    Audio = 1,
    Register = 2,
    Probe = 3,
    ProbeAck = 4,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoicePacket {
    pub kind: PacketType,
    pub flags: u16,
    pub session_id: u64,
    pub sequence: u32,
    pub timestamp: u32,
    pub key_epoch: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PacketError {
    Size,
    MagicOrVersion,
    Type,
    PayloadLength,
}
impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid voice packet: {self:?}")
    }
}
impl std::error::Error for PacketError {}

impl VoicePacket {
    pub fn encode(&self) -> Result<Vec<u8>, PacketError> {
        let mut output = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        self.encode_into(&mut output)?;
        Ok(output)
    }

    pub fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), PacketError> {
        Self::encode_parts_into(
            self.kind,
            self.flags,
            self.session_id,
            self.sequence,
            self.timestamp,
            self.key_epoch,
            &self.payload,
            output,
        )
    }

    pub fn authenticated_header(&self, payload_len: usize) -> Result<Vec<u8>, PacketError> {
        if payload_len > MAX_PACKET_SIZE - HEADER_SIZE {
            return Err(PacketError::Size);
        }
        let mut output = Vec::with_capacity(HEADER_SIZE);
        output.extend(MAGIC.to_be_bytes());
        output.push(VERSION);
        output.push(self.kind as u8);
        output.extend(self.flags.to_be_bytes());
        output.extend(self.session_id.to_be_bytes());
        output.extend(self.sequence.to_be_bytes());
        output.extend(self.timestamp.to_be_bytes());
        output.extend(self.key_epoch.to_be_bytes());
        output.extend((payload_len as u16).to_be_bytes());
        Ok(output)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_parts_into(
        kind: PacketType,
        flags: u16,
        session_id: u64,
        sequence: u32,
        timestamp: u32,
        key_epoch: u16,
        payload: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(), PacketError> {
        if payload.len() > MAX_PACKET_SIZE - HEADER_SIZE {
            return Err(PacketError::Size);
        }
        output.clear();
        output.reserve(HEADER_SIZE + payload.len());
        output.extend(MAGIC.to_be_bytes());
        output.push(VERSION);
        output.push(kind as u8);
        output.extend(flags.to_be_bytes());
        output.extend(session_id.to_be_bytes());
        output.extend(sequence.to_be_bytes());
        output.extend(timestamp.to_be_bytes());
        output.extend(key_epoch.to_be_bytes());
        output.extend((payload.len() as u16).to_be_bytes());
        output.extend(payload);
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PacketError> {
        if !(HEADER_SIZE..=MAX_PACKET_SIZE).contains(&bytes.len()) {
            return Err(PacketError::Size);
        }
        if u16::from_be_bytes([bytes[0], bytes[1]]) != MAGIC || bytes[2] != VERSION {
            return Err(PacketError::MagicOrVersion);
        }
        let kind = match bytes[3] {
            1 => PacketType::Audio,
            2 => PacketType::Register,
            3 => PacketType::Probe,
            4 => PacketType::ProbeAck,
            _ => return Err(PacketError::Type),
        };
        let payload_len = u16::from_be_bytes([bytes[24], bytes[25]]) as usize;
        if payload_len != bytes.len() - HEADER_SIZE {
            return Err(PacketError::PayloadLength);
        }
        Ok(Self {
            kind,
            flags: u16::from_be_bytes([bytes[4], bytes[5]]),
            session_id: u64::from_be_bytes([
                bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
            ]),
            sequence: u32::from_be_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]),
            timestamp: u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]),
            key_epoch: u16::from_be_bytes([bytes[22], bytes[23]]),
            payload: bytes[HEADER_SIZE..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let packet = VoicePacket {
            kind: PacketType::Audio,
            flags: 9,
            session_id: 42,
            sequence: u32::MAX,
            timestamp: 960,
            key_epoch: 7,
            payload: vec![1, 2, 3],
        };
        assert_eq!(
            VoicePacket::decode(&packet.encode().unwrap()).unwrap(),
            packet
        );
    }
    #[test]
    fn rejects_bad_length() {
        let mut bytes = VoicePacket {
            kind: PacketType::Audio,
            flags: 0,
            session_id: 1,
            sequence: 0,
            timestamp: 0,
            key_epoch: 0,
            payload: vec![],
        }
        .encode()
        .unwrap();
        bytes.push(1);
        assert_eq!(VoicePacket::decode(&bytes), Err(PacketError::PayloadLength));
    }

    #[test]
    fn matches_cross_language_golden_wire_format() {
        let packet = VoicePacket {
            kind: PacketType::Audio,
            flags: 0xa1b2,
            session_id: 0x0102_0304_0506_0708,
            sequence: 0x1122_3344,
            timestamp: 0x5566_7788,
            key_epoch: 0,
            payload: vec![0xde, 0xad],
        };
        let expected = [
            0x45, 0x44, 0x02, 0x01, 0xa1, 0xb2, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x00, 0x00, 0x00, 0x02, 0xde, 0xad,
        ];
        assert_eq!(packet.encode().unwrap(), expected);
    }
}
