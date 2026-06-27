use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub id: String,
    pub source: String,
    pub payload: String,
    pub reason: String,
    pub created_at: i64,
}

impl DeadLetterEntry {
    pub fn new(
        id: impl Into<String>,
        source: impl Into<String>,
        payload: impl Into<String>,
        reason: impl Into<String>,
        now_ts: i64,
    ) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            payload: payload.into(),
            reason: reason.into(),
            created_at: now_ts,
        }
    }
}
