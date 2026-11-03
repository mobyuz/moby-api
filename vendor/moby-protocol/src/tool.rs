use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{StableId, ZonedDateTime};

const MAX_QUERY_CHARS: usize = 100;
const MAX_REMINDER_CHARS: usize = 200;
const MAX_SEARCH_RESULTS: u8 = 50;

/// A literal substring matched against file names in a registered search root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "String")]
#[schemars(!try_from, transparent)]
pub struct SearchQuery(
    #[schemars(
        length(min = 1, max = MAX_QUERY_CHARS),
        regex(pattern = r"^(?!\.{1,2}(?![\s\S]))[^/\\:\x00-\x1f\x7f-\x9f]+(?![\s\S])")
    )]
    String,
);

impl SearchQuery {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SearchQuery {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.chars().take(MAX_QUERY_CHARS + 1).count() > MAX_QUERY_CHARS
            || matches!(value.as_str(), "." | "..")
            || value
                .chars()
                .any(|ch| matches!(ch, '/' | '\\' | ':') || ch.is_control())
        {
            return Err(
                "expected a file name substring of 1–100 characters without path separators or control characters",
            );
        }
        Ok(Self(value))
    }
}

/// An inclusive bound on how many file names a search may return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "u8")]
#[schemars(!try_from, transparent)]
pub struct SearchLimit(#[schemars(range(min = 1, max = MAX_SEARCH_RESULTS))] u8);

impl SearchLimit {
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for SearchLimit {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if !(1..=MAX_SEARCH_RESULTS).contains(&value) {
            return Err("expected a search result limit of 1–50");
        }
        Ok(Self(value))
    }
}

/// User-visible reminder content, treated as plain text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "String")]
#[schemars(!try_from, transparent)]
pub struct ReminderText(#[schemars(length(min = 1, max = MAX_REMINDER_CHARS))] String);

impl ReminderText {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReminderText {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.chars().take(MAX_REMINDER_CHARS + 1).count() > MAX_REMINDER_CHARS
        {
            return Err("expected reminder text of 1–200 characters");
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    GitStatus,
    OpenApp,
    OpenProject,
    SearchFiles,
    CreateReminder,
}

/// An untrusted suggestion. The desktop resolves IDs and checks permissions
/// and confirmation before any local action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolRequest {
    GitStatus {
        project_id: StableId,
    },
    OpenApp {
        app_id: StableId,
    },
    OpenProject {
        project_id: StableId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "generate", ts(optional = nullable))]
        app_id: Option<StableId>,
    },
    SearchFiles {
        root_id: StableId,
        query: SearchQuery,
        max_results: SearchLimit,
    },
    CreateReminder {
        text: ReminderText,
        due_at: ZonedDateTime,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub call_id: StableId,
    pub request: ToolRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureCode {
    Denied,
    InvalidRequest,
    Unavailable,
    Canceled,
    Timeout,
    Failed,
    PartialResult,
}

/// A desktop outcome correlated to a call. Treat output as untrusted data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResult {
    Ok {
        call_id: StableId,
        output: String,
    },
    Error {
        call_id: StableId,
        code: ToolFailureCode,
    },
}
