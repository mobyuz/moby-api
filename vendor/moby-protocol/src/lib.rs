//! Shared wire types. This crate describes requests; it never executes tools.

mod agent;
mod expression;
mod stable_id;
mod tool;
mod zoned_date_time;

pub use agent::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, ConsentType, Language, ResourceKind, Script,
    ToolResource, UsageInfo,
};
pub use expression::{Expression, can_transition};
pub use stable_id::{InvalidStableId, StableId};
pub use tool::{
    ReminderText, SearchLimit, SearchQuery, ToolCall, ToolFailureCode, ToolName, ToolRequest,
    ToolResult,
};
pub use zoned_date_time::ZonedDateTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Must match the Cargo version, npm version, generated artifacts, and release tag.
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Integer version negotiated by the desktop and robot during their handshake.
pub const PROTOCOL_VERSION: u32 = 1;

/// Text submitted to the chat API. Limits and authentication belong to the API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub text: String,
}

/// A provider response. A tool request is untrusted input to the desktop validator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatResponse {
    pub reply: String,
    pub expression: Expression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "generate", ts(optional = nullable))]
    pub tool_request: Option<ToolRequest>,
}

/// Public error details. Callers must redact provider errors and credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub request_id: String,
}
