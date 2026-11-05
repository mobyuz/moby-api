use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AudioEnd, AudioPlay, AudioStart, ButtonEvent, Cancel, DeviceConfig, DisplayText, Empty,
    ErrorCode, ErrorDetail, ErrorInfo, Heartbeat, Hello, HelloAck, MuteChanged, PairBegin, PairKey,
    PairSetKey, ServoPose, SetExpression, WakeDetected,
};
use crate::StableId;

impl ControlVersion {
    pub const V1: Self = Self(1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "u32")]
#[schemars(!try_from, transparent)]
pub struct ControlVersion(#[schemars(extend("const" = ControlVersion::V1.0))] u32);

impl TryFrom<u32> for ControlVersion {
    type Error = &'static str;

    fn try_from(version: u32) -> Result<Self, Self::Error> {
        if version == Self::V1.0 {
            Ok(Self::V1)
        } else {
            Err("unsupported USB control envelope version")
        }
    }
}

/// The envelope `kind` of robot notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Event,
}

/// The envelope `kind` of desktop requests. Each one gets a response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ResponseKind {
    Response,
}

impl EventKind {
    pub const WIRE: &'static str = "event";
}

impl CommandKind {
    pub const WIRE: &'static str = "command";
}

impl ResponseKind {
    pub const WIRE: &'static str = "response";
}

/// Who may send a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peer {
    Robot,
    Desktop,
}

impl Peer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Robot => "robot",
            Self::Desktop => "desktop",
        }
    }
}

/// One row of the message catalog that the C header and TypeScript constants
/// are generated from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageInfo {
    pub name: &'static str,
    pub kind: &'static str,
    pub sender: Peer,
}

/// Defines `ControlMessage` and its catalog from one table, so a name, its
/// envelope kind, its sender, and its payload cannot disagree.
macro_rules! control_messages {
    ($($(#[$meta:meta])* $variant:ident = $name:literal, $kind:ident, $sender:ident, $data:ty;)+) => {
        /// Every USB v1 control message. The `name` selects the variant, the
        /// variant fixes the envelope `kind`, and unknown names and fields fail.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[cfg_attr(feature = "generate", derive(ts_rs::TS))]
        #[serde(tag = "name", deny_unknown_fields)]
        pub enum ControlMessage {
            $(
                $(#[$meta])*
                #[serde(rename = $name)]
                $variant {
                    v: ControlVersion,
                    kind: $kind,
                    id: StableId,
                    #[schemars(range(max = u32::MAX))]
                    ts_ms: u32,
                    data: $data,
                },
            )+
        }

        impl ControlMessage {
            pub fn name(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $name,)+
                }
            }

            pub fn sender(&self) -> Peer {
                match self {
                    $(Self::$variant { .. } => Peer::$sender,)+
                }
            }

            pub fn id(&self) -> &StableId {
                match self {
                    $(Self::$variant { id, .. } => id,)+
                }
            }
        }

        /// All control messages in schema order.
        pub const MESSAGES: &[MessageInfo] = &[
            $(MessageInfo { name: $name, kind: $kind::WIRE, sender: Peer::$sender },)+
        ];
    };
}

control_messages! {
    Hello = "hello", EventKind, Robot, Hello;
    Heartbeat = "heartbeat", EventKind, Robot, Heartbeat;
    WakeDetected = "wake_detected", EventKind, Robot, WakeDetected;
    ButtonEvent = "button_event", EventKind, Robot, ButtonEvent;
    MuteChanged = "mute_changed", EventKind, Robot, MuteChanged;
    AudioStart = "audio_start", EventKind, Robot, AudioStart;
    AudioEnd = "audio_end", EventKind, Robot, AudioEnd;
    Error = "error", EventKind, Robot, ErrorInfo;
    /// The talk button was pressed during the pairing window.
    PairButtonConfirmed = "pair_button_confirmed", EventKind, Robot, Empty;
    /// A 10 s long press wiped the robot's pairing key.
    PairReset = "pair_reset", EventKind, Robot, Empty;
    /// The response to `hello`, carrying a proof when the robot is paired.
    HelloAck = "hello_ack", ResponseKind, Desktop, HelloAck;
    Expression = "expression", CommandKind, Desktop, SetExpression;
    DisplayText = "display_text", CommandKind, Desktop, DisplayText;
    AudioPlayStart = "audio_play_start", CommandKind, Desktop, AudioPlay;
    AudioPlayEnd = "audio_play_end", CommandKind, Desktop, AudioPlay;
    Cancel = "cancel", CommandKind, Desktop, Cancel;
    SetConfig = "set_config", CommandKind, Desktop, DeviceConfig;
    ServoPose = "servo_pose", CommandKind, Desktop, ServoPose;
    PairBegin = "pair_begin", CommandKind, Desktop, PairBegin;
    PairSetKey = "pair_set_key", CommandKind, Desktop, PairSetKey;
    /// The robot accepted and applied the command with the same ID.
    CommandOk = "command_ok", ResponseKind, Robot, Empty;
    /// The robot refused or failed the command with the same ID.
    CommandError = "command_error", ResponseKind, Robot, ErrorInfo;
}

impl ControlMessage {
    pub fn is_command(&self) -> bool {
        matches!(
            self,
            Self::Expression { .. }
                | Self::DisplayText { .. }
                | Self::AudioPlayStart { .. }
                | Self::AudioPlayEnd { .. }
                | Self::Cancel { .. }
                | Self::SetConfig { .. }
                | Self::ServoPose { .. }
                | Self::PairBegin { .. }
                | Self::PairSetKey { .. }
        )
    }

    /// Make a correlated response. The desktop supplies a session ID only
    /// when its local admission policy allows the device. Pass the pair key
    /// to prove the session to a robot that reported `paired`; without it, or
    /// for an unpaired robot, the acknowledgement carries no proof.
    pub fn acknowledge_hello(
        &self,
        session_id: Option<StableId>,
        pair_key: Option<&PairKey>,
        ts_ms: u32,
    ) -> Option<Self> {
        let Self::Hello { v, id, data, .. } = self else {
            return None;
        };
        let session_id = session_id.filter(|_| data.supports_protocol());
        let proof = match (pair_key, &session_id) {
            (Some(key), Some(session_id)) if data.paired => {
                Some(key.session_proof(&data.nonce, &data.device_id, session_id))
            }
            _ => None,
        };
        Some(Self::HelloAck {
            v: *v,
            kind: ResponseKind::Response,
            id: id.clone(),
            ts_ms,
            data: HelloAck {
                accepted: session_id.is_some(),
                session_id,
                proof,
                config: DeviceConfig::default(),
            },
        })
    }

    /// Read the session from an accepted response to the expected hello ID.
    pub fn accepted_session(&self, hello_id: &StableId) -> Option<&StableId> {
        match self {
            Self::HelloAck { id, data, .. } if id == hello_id && data.accepted => {
                data.session_id.as_ref()
            }
            _ => None,
        }
    }

    /// The robot's success response to a command. Returns `None` for any
    /// other message.
    pub fn respond_ok(&self, ts_ms: u32) -> Option<Self> {
        self.is_command().then(|| Self::CommandOk {
            v: ControlVersion::V1,
            kind: ResponseKind::Response,
            id: self.id().clone(),
            ts_ms,
            data: Empty {},
        })
    }

    /// The robot's failure response to a command. Returns `None` for any
    /// other message.
    pub fn respond_error(&self, code: ErrorCode, detail: ErrorDetail, ts_ms: u32) -> Option<Self> {
        self.is_command().then(|| Self::CommandError {
            v: ControlVersion::V1,
            kind: ResponseKind::Response,
            id: self.id().clone(),
            ts_ms,
            data: ErrorInfo { code, detail },
        })
    }
}
