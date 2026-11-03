use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Face states remain available locally when the backend is unreachable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Expression {
    #[default]
    Idle,
    Listening,
    Thinking,
    Speaking,
    Happy,
    Confused,
    Error,
    Muted,
    Disconnected,
}

impl Expression {
    /// Stable order for exhaustive validation and generated runtime helpers.
    pub const ALL: [Self; 9] = [
        Self::Idle,
        Self::Listening,
        Self::Thinking,
        Self::Speaking,
        Self::Happy,
        Self::Confused,
        Self::Error,
        Self::Muted,
        Self::Disconnected,
    ];
}

/// Pure transition policy. Repeated state updates are idempotent. Error, mute,
/// and disconnection can interrupt any state. Reconnection/unmute go via idle.
pub const fn can_transition(from: Expression, to: Expression) -> bool {
    use Expression::*;

    if matches!(to, Error | Muted | Disconnected) {
        return true;
    }

    match from {
        Idle => matches!(to, Idle | Listening | Thinking | Happy | Confused),
        Listening => matches!(to, Idle | Listening | Thinking),
        Thinking => matches!(to, Idle | Thinking | Speaking | Happy | Confused),
        Speaking => matches!(to, Idle | Listening | Speaking | Happy | Confused),
        Happy => matches!(to, Idle | Listening | Thinking | Happy),
        Confused => matches!(to, Idle | Listening | Thinking | Confused),
        Error => matches!(to, Idle | Listening | Thinking),
        Muted | Disconnected => matches!(to, Idle),
    }
}
