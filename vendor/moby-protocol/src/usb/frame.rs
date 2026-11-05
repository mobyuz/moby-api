use std::{error::Error, fmt, mem};

pub const FRAME_VERSION: u8 = 1;
pub const DELIMITER: u8 = 0;
pub const HEADER_LEN: usize = 12;
pub const CRC_LEN: usize = 4;
pub const MAX_PAYLOAD_LEN: usize = 1024;
pub const MAX_DECODED_LEN: usize = HEADER_LEN + MAX_PAYLOAD_LEN + CRC_LEN;
pub const MAX_ENCODED_LEN: usize = cobs::max_encoding_length(MAX_DECODED_LEN);
pub const MAX_WIRE_LEN: usize = MAX_ENCODED_LEN + 1;

pub const VERSION_OFFSET: usize = 0;
pub const KIND_OFFSET: usize = 1;
pub const FLAGS_OFFSET: usize = 2;
pub const RESERVED_OFFSET: usize = 3;
pub const STREAM_ID_OFFSET: usize = 4;
pub const SEQUENCE_OFFSET: usize = 6;
pub const LENGTH_OFFSET: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 1,
    RobotAudio = 2,
    DesktopAudio = 3,
}

impl FrameKind {
    fn from_byte(value: u8) -> Result<Self, FrameError> {
        match value {
            value if value == Self::Control as u8 => Ok(Self::Control),
            value if value == Self::RobotAudio as u8 => Ok(Self::RobotAudio),
            value if value == Self::DesktopAudio as u8 => Ok(Self::DesktopAudio),
            _ => Err(FrameError::BadFrame),
        }
    }

    fn valid_stream_id(self, stream_id: u16) -> bool {
        (self == Self::Control) == (stream_id == 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    BadFrame,
    BadCrc,
    UnsupportedVersion(u8),
    FrameTooLarge,
    OutputTooSmall,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadFrame => formatter.write_str("invalid USB frame"),
            Self::BadCrc => formatter.write_str("USB frame CRC mismatch"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported USB frame version {version}")
            }
            Self::FrameTooLarge => formatter.write_str("USB frame exceeds the size limit"),
            Self::OutputTooSmall => {
                formatter.write_str("output buffer is too small for the USB frame")
            }
        }
    }
}

impl Error for FrameError {}

/// The decoded contents of a USB frame. Audio and control semantics are
/// checked by the consumer after framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: FrameKind,
    pub stream_id: u16,
    pub sequence: u32,
    pub payload: &'a [u8],
}

impl Frame<'_> {
    /// Write a complete wire frame, including its zero delimiter. On error
    /// the output buffer may contain partial bytes and must be discarded.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, FrameError> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(FrameError::FrameTooLarge);
        }
        if !self.kind.valid_stream_id(self.stream_id) {
            return Err(FrameError::BadFrame);
        }
        let capacity = output
            .len()
            .checked_sub(1)
            .ok_or(FrameError::OutputTooSmall)?;

        let mut raw = [0; MAX_DECODED_LEN];
        raw[VERSION_OFFSET] = FRAME_VERSION;
        raw[KIND_OFFSET] = self.kind as u8;
        raw[STREAM_ID_OFFSET..SEQUENCE_OFFSET].copy_from_slice(&self.stream_id.to_le_bytes());
        raw[SEQUENCE_OFFSET..LENGTH_OFFSET].copy_from_slice(&self.sequence.to_le_bytes());
        raw[LENGTH_OFFSET..HEADER_LEN].copy_from_slice(&(self.payload.len() as u16).to_le_bytes());
        let body_len = HEADER_LEN + self.payload.len();
        raw[HEADER_LEN..body_len].copy_from_slice(self.payload);
        let checksum = crc32fast::hash(&raw[..body_len]);
        raw[body_len..body_len + CRC_LEN].copy_from_slice(&checksum.to_le_bytes());

        let written = cobs::try_encode(&raw[..body_len + CRC_LEN], &mut output[..capacity])
            .map_err(|_| FrameError::OutputTooSmall)?;
        output[written] = DELIMITER;
        Ok(written + 1)
    }
}

fn parse_frame(raw: &[u8]) -> Result<Frame<'_>, FrameError> {
    let header = raw.get(..HEADER_LEN).ok_or(FrameError::BadFrame)?;
    let payload_len =
        u16::from_le_bytes([header[LENGTH_OFFSET], header[LENGTH_OFFSET + 1]]) as usize;
    let body_len = HEADER_LEN + payload_len;
    if payload_len > MAX_PAYLOAD_LEN || raw.len() != body_len + CRC_LEN {
        return Err(FrameError::BadFrame);
    }
    let checksum = u32::from_le_bytes(raw[body_len..].try_into().expect("fixed CRC width"));
    if crc32fast::hash(&raw[..body_len]) != checksum {
        return Err(FrameError::BadCrc);
    }
    if header[VERSION_OFFSET] != FRAME_VERSION {
        return Err(FrameError::UnsupportedVersion(header[VERSION_OFFSET]));
    }
    if header[FLAGS_OFFSET] != 0 || header[RESERVED_OFFSET] != 0 {
        return Err(FrameError::BadFrame);
    }
    let kind = FrameKind::from_byte(header[KIND_OFFSET])?;
    let stream_id = u16::from_le_bytes([header[STREAM_ID_OFFSET], header[STREAM_ID_OFFSET + 1]]);
    if !kind.valid_stream_id(stream_id) {
        return Err(FrameError::BadFrame);
    }
    Ok(Frame {
        kind,
        stream_id,
        sequence: u32::from_le_bytes(
            header[SEQUENCE_OFFSET..LENGTH_OFFSET]
                .try_into()
                .expect("fixed sequence width"),
        ),
        payload: &raw[HEADER_LEN..body_len],
    })
}

/// A bounded stream decoder. A zero delimiter ends the current segment
/// and restores synchronization even when that segment was invalid.
pub struct FrameDecoder {
    encoded: [u8; MAX_ENCODED_LEN],
    decoded: [u8; MAX_DECODED_LEN],
    len: usize,
    dropping: bool,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self {
            encoded: [0; MAX_ENCODED_LEN],
            decoded: [0; MAX_DECODED_LEN],
            len: 0,
            dropping: false,
        }
    }
}

impl FrameDecoder {
    /// Discard a partial frame when the serial connection is closed.
    pub fn reset(&mut self) {
        self.len = 0;
        self.dropping = false;
    }

    /// Feed one serial byte. Idle delimiters return no frame.
    pub fn push(&mut self, byte: u8) -> Option<Result<Frame<'_>, FrameError>> {
        if byte != DELIMITER {
            if self.len == MAX_ENCODED_LEN {
                self.dropping = true;
            } else {
                self.encoded[self.len] = byte;
                self.len += 1;
            }
            return None;
        }

        let len = mem::take(&mut self.len);
        if mem::take(&mut self.dropping) {
            return Some(Err(FrameError::FrameTooLarge));
        }
        if len == 0 {
            return None;
        }
        let decoded = match cobs::decode(&self.encoded[..len], &mut self.decoded) {
            Ok(report) if report.parsed_size() == len => report.frame_size(),
            _ => return Some(Err(FrameError::BadFrame)),
        };
        Some(parse_frame(&self.decoded[..decoded]))
    }
}
