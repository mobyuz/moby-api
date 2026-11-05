use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApiError, Expression, StableId, ToolCall, ToolName, ToolResult, ZonedDateTime};

const MAX_LOCAL_TIME_CHARS: usize = 35;

/// The user's current time as an RFC 3339 timestamp with a mandatory UTC
/// offset, such as `2026-11-05T09:30:00+05:00`. It uses the same format as the
/// reminder `due_at` and is at most 35 characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "String")]
#[schemars(!try_from, transparent)]
pub struct LocalTime(
    #[schemars(
        length(max = MAX_LOCAL_TIME_CHARS),
        extend("format" = "date-time"),
        regex(pattern = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-5][0-9](?:\.[0-9]{1,9})?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})(?![\s\S])")
    )]
    String,
);

impl LocalTime {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LocalTime {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.chars().take(MAX_LOCAL_TIME_CHARS + 1).count() > MAX_LOCAL_TIME_CHARS {
            return Err("expected a local time of at most 35 characters");
        }
        // The due_at check decides what a valid zoned timestamp is.
        ZonedDateTime::try_from(value).map(|time| Self(time.as_str().to_owned()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Uz,
    Ru,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Script {
    Latn,
    Cyrl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConsentType {
    Terms,
    PrivacyPolicy,
    CloudVoice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UsageInfo {
    #[schemars(range(max = u32::MAX))]
    pub llm_tokens_remaining: u32,
    #[schemars(range(max = u32::MAX))]
    pub stt_seconds_remaining: u32,
    #[schemars(range(max = u32::MAX))]
    pub tts_chars_remaining: u32,
    pub reset_at: ZonedDateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    App,
    Project,
    SearchRoot,
}

/// Public metadata for a resource registered on the desktop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ToolResource {
    pub kind: ResourceKind,
    pub id: StableId,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMessage {
    User { text: String },
    Assistant { text: String },
    ToolCall { call: ToolCall },
    ToolResult { result: ToolResult },
}

/// Context for a stateless turn. Locale and time fields are optional hints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AgentTurnRequest {
    pub messages: Vec<AgentMessage>,
    pub available_tools: Vec<ToolName>,
    pub resources: Vec<ToolResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub language: Option<Language>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub script: Option<Script>,
    /// The user's current time, so the model can resolve relative reminder
    /// times. Absent when the desktop cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub local_time: Option<LocalTime>,
}

/// One event from an agent turn. A speech delta can precede a terminal
/// completion. Tool calls, completions, and errors end the current request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentTurnResponse {
    SpeechDelta {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    Completed {
        speech_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "generate", ts(optional = nullable))]
        display_text: Option<String>,
        expression_hint: Expression,
    },
    Error {
        error: ApiError,
    },
}
