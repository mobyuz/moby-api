//! Host contract for the candidate USB v1 wire format.

mod bounded;
mod control;
mod frame;
mod message;
mod pairing;

pub use bounded::{
    ActionDurationMs, AudioStreamId, ChannelCount, ErrorDetail, FrameMs, MaxUtteranceMs,
    PairTimeoutMs, Percent, PrerollMs, SampleRate, ScreenText, TextTtlMs, VadSilenceMs,
    VersionText,
};
pub use control::{
    CommandKind, ControlMessage, ControlVersion, EventKind, MESSAGES, MessageInfo, Peer,
    ResponseKind,
};
pub use frame::{
    CRC_LEN, DELIMITER, FLAGS_OFFSET, FRAME_VERSION, Frame, FrameDecoder, FrameError, FrameKind,
    HEADER_LEN, KIND_OFFSET, LENGTH_OFFSET, MAX_DECODED_LEN, MAX_ENCODED_LEN, MAX_PAYLOAD_LEN,
    MAX_WIRE_LEN, RESERVED_OFFSET, SEQUENCE_OFFSET, STREAM_ID_OFFSET, VERSION_OFFSET,
};
pub use message::{
    AudioCodec, AudioEnd, AudioEndReason, AudioPlay, AudioStart, AudioStartReason, Button,
    ButtonAction, ButtonEvent, Cancel, Capabilities, DeviceConfig, DisplayText, Empty, ErrorCode,
    ErrorInfo, Heartbeat, Hello, HelloAck, MuteChanged, MuteSource, PairBegin, PairSetKey,
    ServoPose, SetExpression, WakeDetected,
};
pub use pairing::{Nonce, PairKey, Proof, session_proof_input};
