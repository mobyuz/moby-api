use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApiError, Expression, StableId, ToolCall, ToolName, ToolResult, ZonedDateTime};

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

/// Context for a stateless turn. Locale fields are optional hints.
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
