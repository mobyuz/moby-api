//! The `data` object of each control message. Every struct rejects unknown
//! fields; every number and text is bounded by `bounded.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ActionDurationMs, AudioStreamId, ChannelCount, ErrorDetail, FrameMs, MaxUtteranceMs, Nonce,
    PairKey, PairTimeoutMs, Percent, PrerollMs, Proof, SampleRate, ScreenText, TextTtlMs,
    VadSilenceMs, VersionText,
};
use crate::{Expression, PROTOCOL_VERSION, StableId};

/// The empty `data` object of messages that carry no fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// Robot error codes. The same codes appear in `error` events and in
/// `command_error` responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
pub enum ErrorCode {
    #[serde(rename = "E_PROTO_VERSION")]
    ProtoVersion,
    #[serde(rename = "E_BAD_FRAME")]
    BadFrame,
    #[serde(rename = "E_CRC")]
    Crc,
    #[serde(rename = "E_STATE")]
    State,
    #[serde(rename = "E_MUTED")]
    Muted,
    #[serde(rename = "E_BUSY")]
    Busy,
    #[serde(rename = "E_AUDIO_OVERRUN")]
    AudioOverrun,
    #[serde(rename = "E_TIMEOUT")]
    Timeout,
    #[serde(rename = "E_UNSUPPORTED")]
    Unsupported,
    #[serde(rename = "E_HW")]
    Hw,
}

impl ErrorCode {
    /// Stable order for generated runtime helpers.
    pub const ALL: [Self; 10] = [
        Self::ProtoVersion,
        Self::BadFrame,
        Self::Crc,
        Self::State,
        Self::Muted,
        Self::Busy,
        Self::AudioOverrun,
        Self::Timeout,
        Self::Unsupported,
        Self::Hw,
    ];
}

/// The payload of an `error` event and of a `command_error` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub detail: ErrorDetail,
}

/// Optional device settings. The robot applies the fields present and keeps
/// the rest. `hello_ack` and `set_config` share this type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub wake_sensitivity: Option<Percent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub speaker_volume: Option<Percent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub vad_silence_ms: Option<VadSilenceMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub max_utterance_ms: Option<MaxUtteranceMs>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub device_id: StableId,
    pub hw_rev: VersionText,
    pub fw_version: VersionText,
    #[schemars(range(max = u32::MAX))]
    pub protocol_version: u32,
    pub capabilities: Capabilities,
    pub mute: bool,
    /// Whether the robot holds a pairing key.
    pub paired: bool,
    /// Fresh for every hello, so a recorded proof cannot be replayed.
    pub nonce: Nonce,
}

/// Capability IDs a robot advertises: at most 8, so the longest hello fits one frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "Vec<StableId>")]
#[schemars(!try_from, transparent)]
pub struct Capabilities(#[schemars(length(max = 8))] Vec<StableId>);

impl Capabilities {
    pub const MAX: usize = 8;

    pub fn as_slice(&self) -> &[StableId] {
        &self.0
    }
}

impl TryFrom<Vec<StableId>> for Capabilities {
    type Error = &'static str;

    fn try_from(value: Vec<StableId>) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX {
            return Err("expected at most 8 capabilities");
        }
        Ok(Self(value))
    }
}

impl Hello {
    pub fn supports_protocol(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct HelloAck {
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub session_id: Option<StableId>,
    /// Present only when the robot reported `paired` and the desktop holds
    /// the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub proof: Option<Proof>,
    pub config: DeviceConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub state: Expression,
    pub mute: bool,
    #[schemars(range(max = u32::MAX))]
    pub uptime_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WakeDetected {
    pub model_id: StableId,
    pub model_version: VersionText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Talk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    Press,
    Release,
    /// Held for the robot's long-press time. A 10 s hold wipes pairing.
    LongPress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ButtonEvent {
    pub button: Button,
    pub action: ButtonAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MuteSource {
    /// The user moved the hardware mute control.
    Switch,
    /// The robot reports the control's position at startup.
    Boot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MuteChanged {
    pub muted: bool,
    pub source: MuteSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AudioStartReason {
    Wake,
    Button,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    /// Signed 16-bit little-endian PCM.
    PcmS16le,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AudioStart {
    pub stream_id: AudioStreamId,
    pub reason: AudioStartReason,
    pub codec: AudioCodec,
    pub sample_rate: SampleRate,
    pub channels: ChannelCount,
    pub frame_ms: FrameMs,
    pub preroll_ms: PrerollMs,
}

impl AudioStart {
    /// Bytes of PCM in one frame. The sender chooses parameters so this stays
    /// within `MAX_PAYLOAD_LEN`; the schema cannot express that relation.
    pub fn frame_bytes(&self) -> usize {
        let samples = self.sample_rate.get() as usize * usize::from(self.frame_ms.get()) / 1000;
        samples * usize::from(self.channels.get()) * 2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AudioEndReason {
    VadSilence,
    MaxDuration,
    ButtonRelease,
    Mute,
    Cancel,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AudioEnd {
    pub stream_id: AudioStreamId,
    pub reason: AudioEndReason,
    #[schemars(range(max = u32::MAX))]
    pub frames: u32,
    #[schemars(range(max = u32::MAX))]
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SetExpression {
    pub state: Expression,
    /// An animation the firmware knows by name. Unknown names get `E_UNSUPPORTED`.
    pub animation: StableId,
    pub duration_ms: ActionDurationMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DisplayText {
    pub text: ScreenText,
    pub ttl_ms: TextTtlMs,
}

/// Describes the desktop's audio for `audio_play_start` and `audio_play_end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AudioPlay {
    pub stream_id: AudioStreamId,
    pub codec: AudioCodec,
    pub sample_rate: SampleRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub stream_id: AudioStreamId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ServoPose {
    /// A pose the firmware knows by name. Unknown names get `E_UNSUPPORTED`.
    pub pose: StableId,
    pub duration_ms: ActionDurationMs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PairBegin {
    pub timeout_ms: PairTimeoutMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PairSetKey {
    pub pair_key: PairKey,
}
